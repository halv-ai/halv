use crate::{
    ponytail::PonytailMode,
    pricing::{self, PricingSource},
    savings::{self, AgentSessionTurnTotalsRow, DailyTokenActivity, SavingsToday, SavingsTotals},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Duration, Local, Utc};
use rand::random;
use rusqlite::{params, types::Type, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri_plugin_dialog::DialogExt;

pub(crate) mod routing_savings;
pub(crate) mod savings_dollars;
pub(crate) mod session_token;
mod workspace_tabs;

const DB_FILENAME: &str = "halv.sqlite3";
const MIGRATION_INITIAL: &str = "0001_initial";
const MIGRATION_CONVERSATION_GOAL: &str = "0002_conversation_goal";
const MIGRATION_PROJECTS: &str = "0003_projects";
const MIGRATION_AGENT_MODE_TRUST: &str = "0004_agent_mode_trust";
const MIGRATION_APPROVAL_POLICY: &str = "0005_approval_policy";
const MIGRATION_TURN_DIFFS: &str = "0006_turn_diffs";
const MIGRATION_MESSAGE_MENTIONS: &str = "0007_message_mentions";
const MIGRATION_APPROVAL_POLICY_SKIP_ALL: &str = "0008_approval_policy_skip_all";
const MIGRATION_AUTH_SESSION: &str = "0009_auth_session";
const MIGRATION_SEARCH_AND_TITLES: &str = "0010_search_and_titles";
const MIGRATION_TURN_SNAPSHOT_POSTCONDITIONS: &str = "0011_turn_snapshot_postconditions";
const MIGRATION_WORKSPACES: &str = "0012_workspaces";
const MIGRATION_ARCHIVED_SAVINGS: &str = "0013_archived_savings";
const MIGRATION_MESSAGE_ATTACHMENTS: &str = "0014_message_attachments";
const MIGRATION_CONVERSATION_WORKFLOW: &str = "0015_conversation_workflow";
const MIGRATION_SUBAGENT_THREADS: &str = "0016_subagent_threads";
const MIGRATION_WORKSPACE_LOCAL_ENVIRONMENT: &str = "0017_workspace_local_environment";
const MIGRATION_CONVERSATION_WORKTREE_HANDOFF: &str = "0018_conversation_worktree_handoff";
const MIGRATION_CONVERSATION_COMPACTIONS: &str = "0019_conversation_compactions";
const MIGRATION_APPROVAL_POLICY_ON_FAILURE: &str = "0020_approval_policy_on_failure";
const MIGRATION_CONVERSATION_ARCHIVE: &str = "0021_conversation_archive";
const MIGRATION_AGENT_MEMORIES: &str = "0022_agent_memories";
const MIGRATION_CONVERSATION_FAST_MODE: &str = "0023_conversation_fast_mode";
const MIGRATION_LOCAL_DIAGNOSTICS: &str = "0024_local_diagnostics";
const MIGRATION_SAVINGS_REPORT_QUEUE: &str = "0025_savings_report_queue";
const MIGRATION_CRUX_INDEX_STATE: &str = "0026_crux_index_state";
const MIGRATION_CRUX_SAVINGS: &str = "0027_crux_savings";
const MIGRATION_CLEAR_PROJECT_APPROVAL_POLICIES: &str = "0028_clear_project_approval_policies";
const MIGRATION_FREE_TURNS: &str = "0029_free_turns";
pub const MIGRATION_CREDENTIAL_PROFILES: &str = "0030_credential_profiles";
pub const MIGRATION_USAGE_WINDOWS: &str = "0031_usage_windows";
const MIGRATION_AGENT_SESSIONS: &str = "0032_agent_sessions";
const MIGRATION_CONVERSATION_AGENT_BACKEND: &str = "0033_conversation_agent_backend";
const MIGRATION_AGENT_MESSAGE_USAGE: &str = "0034_agent_message_usage";
const MIGRATION_AGENT_SESSION_CUSTOM_TITLE: &str = "0035_agent_session_custom_title";
const MIGRATION_AGENT_SESSION_CONVERSATION: &str = "0036_agent_session_conversation";
const MIGRATION_CONVERSATION_MODE_AGENT_TITLES: &str = "0037_conversation_mode_agent_titles";
const MIGRATION_FIRST_LAUNCH_AT: &str = "0038_first_launch_at";
const MIGRATION_CONVERSATION_PINNED: &str = "0039_conversation_pinned";
const MIGRATION_CONVERSATION_DRAFT: &str = "0040_conversation_draft";
const MIGRATION_AGENT_SESSION_CRUX_SAVINGS: &str = "0042_agent_session_crux_savings";
const MIGRATION_PONYTAIL_MODE: &str = "0041_ponytail_mode";
const MIGRATION_SAVINGS_USD_PRICING: &str = "0043_savings_usd_pricing";
const MIGRATION_AGENT_SESSION_MESSAGE_SNAPSHOT: &str = "0044_agent_session_message_snapshot";
const MIGRATION_SAVINGS_LOCAL_DATE: &str = "0045_savings_occurred_local_date";
const MIGRATION_AGENT_SESSION_SAVINGS_PATHS: &str = "0046_agent_session_savings_paths";
const MIGRATION_SAVINGS_REPORT_USD: &str = "0047_savings_report_usd";
const MIGRATION_SESSION_CHECKPOINTS: &str = "0048_session_checkpoints";
const MIGRATION_TASK_ROUTES: &str = "0049_task_routes";
const MIGRATION_TASK_ROUTES_WORKER_LANE: &str = "0050_task_routes_worker_lane";
const MIGRATION_TASK_ROUTES_EFFORT: &str = "0051_task_routes_effort";
const MIGRATION_TASK_HIERARCHIES: &str = "0052_task_hierarchies";
const MIGRATION_REMOTE_DEVICES: &str = "0053_remote_devices";
const MIGRATION_LOCAL_ENVIRONMENT_APPROVALS: &str = "0055_local_environment_approvals";
const MIGRATION_TASK_ROUTE_RECEIPTS: &str = "0056_task_route_receipts";
const MIGRATION_AGENT_SESSION_HEADROOM_ROUTE: &str = "0060_agent_session_headroom_route";
const DEFAULT_STORAGE_MODEL: &str = "gpt-5.5";
const MAX_CONVERSATION_DRAFT_CHARS: usize = 100_000;
const MAX_AGENT_MEMORIES_PER_SCOPE: usize = 50;
const SEARCH_LIMIT: usize = 50;
const SEARCH_HIGHLIGHT_START: &str = "[[HALV_HIT]]";
const SEARCH_HIGHLIGHT_END: &str = "[[/HALV_HIT]]";
/// The legacy flat price of rows saved before ADR 0015 without a dollar value.
pub(crate) fn savings_usd_for_tokens(tokens_saved: i64) -> f64 {
    pricing::legacy::fallback_input_price().savings_usd(tokens_saved)
}

pub const MIGRATIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS migrations (
    version TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL
);
"#;

pub const INITIAL_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'developer', 'system')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    model TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    error INTEGER NOT NULL DEFAULT 0 CHECK (error IN (0, 1)),
    mentions TEXT,
    FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS savings_snapshots (
    message_id TEXT PRIMARY KEY,
    tokens_saved INTEGER,
    savings_usd REAL,
    pricing_source TEXT NULL CHECK (
        pricing_source IN ('headroom', 'model_table', 'fallback')
            OR pricing_source IS NULL
    ),
    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_conversations_updated_at
    ON conversations(updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_created_at
    ON messages(conversation_id, created_at ASC, id ASC);
"#;

pub const MESSAGE_ATTACHMENTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS message_attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes > 0),
    data BLOB NOT NULL,
    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE,
    UNIQUE(message_id, position)
);

CREATE INDEX IF NOT EXISTS idx_message_attachments_message_position
    ON message_attachments(message_id, position ASC);
"#;

pub const CONVERSATION_WORKFLOW_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_task_plans (
    conversation_id TEXT PRIMARY KEY,
    explanation TEXT,
    plan_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
"#;

pub const CONVERSATION_COMPACTIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_compactions (
    id INTEGER PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    upto_message_id INTEGER NOT NULL,
    summary TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_conversation_compactions_conversation_id
    ON conversation_compactions(conversation_id, id DESC);
"#;

pub const SUBAGENT_THREADS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS subagent_threads (
    agent_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    task_name TEXT NOT NULL,
    task TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'running', 'completed', 'errored', 'interrupted')),
    result TEXT,
    error TEXT,
    generation INTEGER NOT NULL DEFAULT 1 CHECK (generation > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
    UNIQUE(conversation_id, task_name)
);

CREATE TABLE IF NOT EXISTS subagent_messages (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'error', 'system')),
    content TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    created_at TEXT NOT NULL,
    FOREIGN KEY(agent_id) REFERENCES subagent_threads(agent_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_subagent_threads_conversation_updated
    ON subagent_threads(conversation_id, updated_at ASC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_subagent_messages_agent_created
    ON subagent_messages(agent_id, created_at ASC, id ASC);
"#;

pub const CONVERSATION_GOAL_SQL: &str = r#"
ALTER TABLE conversations ADD COLUMN goal TEXT;
"#;

pub const CONVERSATION_ARCHIVE_SQL: &str = r#"
ALTER TABLE conversations ADD COLUMN archived_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_conversations_archived_at
    ON conversations(archived_at DESC);
"#;

pub const AGENT_MEMORIES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_memories (
    id INTEGER PRIMARY KEY,
    project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    source_conversation_id TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_memories_scope_updated
    ON agent_memories(project_id, updated_at DESC, id DESC);
"#;

pub const CONVERSATION_FAST_MODE_SQL: &str = r#"
ALTER TABLE conversations ADD COLUMN fast_mode INTEGER
    CHECK (fast_mode IN (0, 1));
"#;

pub const LOCAL_DIAGNOSTICS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS local_diagnostics (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    first_activated_at TEXT NOT NULL,
    last_activated_at TEXT NOT NULL,
    launch_count INTEGER NOT NULL CHECK (launch_count > 0)
);
"#;

pub const SAVINGS_REPORT_QUEUE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS savings_report_queue (
    message_id TEXT PRIMARY KEY,
    tokens_saved INTEGER NOT NULL CHECK (tokens_saved > 0),
    savings_usd REAL,
    created_at TEXT NOT NULL,
    reported_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_savings_report_queue_unreported
    ON savings_report_queue(created_at ASC, message_id ASC)
    WHERE reported_at IS NULL;
"#;

pub const CRUX_INDEX_STATE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS crux_index_state (
    project_path TEXT PRIMARY KEY,
    last_indexed_git_head TEXT,
    last_index_stats TEXT NOT NULL,
    last_index_time TEXT NOT NULL
);
"#;

pub const AGENT_SESSIONS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_sessions (
  id TEXT PRIMARY KEY,
  agent_kind TEXT NOT NULL,
  lane TEXT NOT NULL,
  mode TEXT NOT NULL DEFAULT 'gui' CHECK (
    mode IN ('gui', 'terminal_embedded', 'terminal_ghostty')
  ),
  project_id INTEGER NULL,
  conversation_id TEXT NULL REFERENCES conversations(id) ON DELETE SET NULL,
  cwd TEXT NOT NULL,
  cli_session_id TEXT NULL,
  rtk_db_path TEXT NULL,
  crux_ledger_path TEXT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  last_active_at TEXT NOT NULL,
  exit_code INTEGER NULL
);

CREATE TABLE IF NOT EXISTS agent_session_turns (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  turn_index INTEGER NOT NULL,
  input_tokens INTEGER NOT NULL,
  output_tokens INTEGER NOT NULL,
  cache_read_tokens INTEGER NOT NULL,
  cache_creation_tokens INTEGER NOT NULL,
  cost_usd REAL NULL,
  savings_tokens INTEGER NULL,
  savings_usd REAL NULL,
  pricing_source TEXT NULL CHECK (
    pricing_source IN ('headroom', 'model_table', 'fallback')
      OR pricing_source IS NULL
  ),
  crux_tokens_saved INTEGER NOT NULL DEFAULT 0 CHECK (crux_tokens_saved >= 0),
  savings_source TEXT NULL CHECK (
    savings_source IN ('measured', 'estimated') OR savings_source IS NULL
  ),
  has_message_snapshot INTEGER NULL CHECK (has_message_snapshot IN (0, 1)),
  created_at TEXT NOT NULL,
  FOREIGN KEY(session_id) REFERENCES agent_sessions(id) ON DELETE CASCADE,
  UNIQUE(session_id, turn_index)
);

CREATE INDEX IF NOT EXISTS idx_agent_session_turns_session
  ON agent_session_turns(session_id, turn_index ASC);
"#;

pub const SESSION_CHECKPOINTS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS session_checkpoints (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  turn_id TEXT NULL,
  repo_root TEXT NOT NULL,
  commit_sha TEXT NOT NULL,
  tree_sha TEXT NOT NULL,
  head_sha TEXT NULL,
  branch TEXT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('turn', 'safety')),
  label TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_session_checkpoints_session_created
  ON session_checkpoints(session_id, created_at DESC, id DESC);
"#;

pub const TASK_ROUTES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS task_routes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    conversation_id TEXT NULL,
    title TEXT NOT NULL,
    brief_chars INTEGER NOT NULL,
    complexity TEXT NOT NULL,
    complexity_confidence REAL NOT NULL,
    needs_reasoning REAL NOT NULL,
    risky REAL NOT NULL,
    tier TEXT NOT NULL,
    lane TEXT NOT NULL,
    model TEXT NULL,
    effort TEXT NULL,
    reasons_json TEXT NOT NULL,
    worker_lane TEXT NOT NULL DEFAULT 'auto',
    status TEXT NOT NULL CHECK (status IN ('routed', 'passed', 'failed', 'escalated')),
    escalated_from_id TEXT NULL,
    provider TEXT NOT NULL,
    provider_model TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS task_routes_session_idx ON task_routes(session_id, created_at);

CREATE INDEX IF NOT EXISTS task_routes_conversation_idx
    ON task_routes(conversation_id, created_at);
"#;

/// The receipts of workflow routes (ADR 0014). A receipt holds the outcome
/// and the token use of one route. An agent row holds the measured use of one
/// workflow agent on one model, so a later cost estimate can price each model.
/// `task_route_agents` is the one cost table of the dollar-savings design: it
/// keeps that design's column names, and the estimator fills `actual_usd` and
/// `price_version`.
pub const TASK_ROUTE_RECEIPTS_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS task_routes_batch_idx ON task_routes(batch_id);

CREATE TABLE IF NOT EXISTS task_route_receipts (
    route_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'escalated')),
    agents INTEGER NOT NULL DEFAULT 0,
    failed_agents INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NULL,
    output_tokens INTEGER NULL,
    cache_read_tokens INTEGER NULL,
    cache_creation_tokens INTEGER NULL,
    duration_ms INTEGER NULL,
    usage_source TEXT NOT NULL CHECK (usage_source IN ('measured', 'reported', 'none')),
    reported_at TEXT NOT NULL,
    FOREIGN KEY(route_id) REFERENCES task_routes(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS task_route_receipts_session_idx
    ON task_route_receipts(session_id);

CREATE TABLE IF NOT EXISTS task_route_agents (
    route_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    observed_model TEXT NOT NULL,
    workflow_run_id TEXT NOT NULL,
    label TEXT NOT NULL,
    observed_effort TEXT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
    actual_usd REAL NULL,
    price_version TEXT NULL,
    usage_source TEXT NOT NULL DEFAULT 'claude_transcript',
    started_at TEXT NULL,
    ended_at TEXT NULL,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (route_id, agent_id, observed_model),
    FOREIGN KEY(route_id) REFERENCES task_routes(id) ON DELETE CASCADE
);
"#;

/// The columns that migration 0056 adds to `task_routes`.
const TASK_ROUTE_RECEIPT_ROUTE_COLUMNS: [(&str, &str); 9] = [
    ("batch_id", "TEXT NULL"),
    ("workflow_name", "TEXT NULL"),
    ("route_key", "TEXT NULL"),
    ("role", "TEXT NULL"),
    ("instances", "INTEGER NOT NULL DEFAULT 1"),
    ("default_model", "TEXT NULL"),
    ("default_effort", "TEXT NULL"),
    ("default_source", "TEXT NULL"),
    ("closed_unreported", "INTEGER NOT NULL DEFAULT 0"),
];

pub const AGENT_MESSAGE_USAGE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_message_usage (
    message_id TEXT PRIMARY KEY,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    savings_source TEXT NOT NULL CHECK (savings_source IN ('measured', 'usage')),
    FOREIGN KEY(message_id) REFERENCES messages(id) ON DELETE CASCADE
);
"#;

pub const PONYTAIL_MODE_SQL: &str = r#"
ALTER TABLE agent_sessions ADD COLUMN ponytail_mode TEXT NULL
  CHECK (ponytail_mode IN ('off', 'lite', 'full', 'ultra') OR ponytail_mode IS NULL);
ALTER TABLE agent_session_turns ADD COLUMN ponytail_mode TEXT NULL
  CHECK (ponytail_mode IN ('off', 'lite', 'full', 'ultra') OR ponytail_mode IS NULL);
ALTER TABLE messages ADD COLUMN ponytail_mode TEXT NULL
  CHECK (ponytail_mode IN ('off', 'lite', 'full', 'ultra') OR ponytail_mode IS NULL);
"#;

pub const AUTH_SESSION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS auth_session (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    server_url TEXT NOT NULL,
    token TEXT,
    email TEXT,
    plan TEXT,
    entitlements_json TEXT,
    synced_at TEXT
);
"#;

pub const CREDENTIAL_PROFILES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS credential_profiles (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'active'
    CHECK (status IN ('active', 'disabled', 'invalid')),
  last_validated_at TEXT,
  metadata_json TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
"#;

pub const USAGE_WINDOWS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS usage_windows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    credential_profile_id TEXT NULL
        REFERENCES credential_profiles(id) ON DELETE CASCADE,
    model TEXT NULL,
    window_kind TEXT NOT NULL,
    limit_value INTEGER NULL,
    remaining INTEGER NULL,
    used INTEGER NULL,
    resets_at TEXT NULL,
    retry_after_seconds INTEGER NULL,
    source TEXT NOT NULL CHECK (source IN ('headers', 'error', 'none')),
    confidence TEXT NOT NULL CHECK (confidence IN ('known', 'estimated', 'unknown')),
    captured_at TEXT NOT NULL,
    UNIQUE(provider, credential_profile_id, model, window_kind)
);

CREATE TABLE IF NOT EXISTS profile_cooldowns (
    provider TEXT NOT NULL,
    credential_profile_id TEXT NULL
        REFERENCES credential_profiles(id) ON DELETE CASCADE,
    reason TEXT NOT NULL,
    next_attempt_at TEXT NULL,
    last_error TEXT NULL,
    recorded_at TEXT NOT NULL,
    UNIQUE(provider, credential_profile_id)
);

-- SQLite considers NULL values distinct in table-level UNIQUE constraints.
-- These indexes preserve one latest row for unpinned profiles and models.
CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_windows_normalized_key
ON usage_windows(
    provider,
    COALESCE(credential_profile_id, ''),
    COALESCE(model, ''),
    window_kind
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_profile_cooldowns_normalized_key
ON profile_cooldowns(provider, COALESCE(credential_profile_id, ''));

CREATE INDEX IF NOT EXISTS idx_usage_windows_provider_profile
ON usage_windows(provider, credential_profile_id);

CREATE INDEX IF NOT EXISTS idx_profile_cooldowns_provider_profile
ON profile_cooldowns(provider, credential_profile_id);
"#;

pub const MESSAGE_SEARCH_FTS_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
USING fts5(
    content,
    content='messages',
    content_rowid='rowid',
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_ai
AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_ad
AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS messages_fts_au
AFTER UPDATE OF content ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
    INSERT INTO messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;
"#;

pub const WORKSPACES_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_opened_at TEXT,
    trust_level TEXT CHECK (trust_level IN ('trusted', 'untrusted')),
    approval_policy TEXT
        CHECK (approval_policy IN ('read_only', 'ask', 'on_request', 'on_failure', 'never', 'skip_all'))
);

CREATE TABLE IF NOT EXISTS workspaces (
    id TEXT PRIMARY KEY,
    project_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('primary', 'worktree')),
    root_path TEXT NOT NULL UNIQUE,
    git_common_dir TEXT,
    base_ref TEXT,
    base_commit TEXT,
    branch TEXT,
    local_environment_config_path TEXT,
    managed INTEGER NOT NULL DEFAULT 0 CHECK (managed IN (0, 1)),
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1)),
    state TEXT NOT NULL DEFAULT 'available'
        CHECK (state IN ('available', 'missing', 'removing')),
    created_at TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_workspaces_primary_project
    ON workspaces(project_id) WHERE kind = 'primary';
CREATE INDEX IF NOT EXISTS idx_workspaces_project_last_used
    ON workspaces(project_id, last_used_at DESC, created_at DESC);
"#;

// Each row approves the scripts of one environment file for one project.
// The approval is valid only while the file content has this SHA-256.
pub const LOCAL_ENVIRONMENT_APPROVALS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS local_environment_approvals (
    project_id INTEGER NOT NULL,
    config_path TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    approved_at TEXT NOT NULL,
    PRIMARY KEY (project_id, config_path),
    FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
);
"#;

pub const ARCHIVED_SAVINGS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS archived_savings_snapshots (
    message_id TEXT PRIMARY KEY,
    message_role TEXT NOT NULL
        CHECK (message_role IN ('user', 'assistant', 'developer', 'system')),
    tokens_saved INTEGER,
    savings_usd REAL,
    pricing_source TEXT NULL CHECK (
        pricing_source IN ('headroom', 'model_table', 'fallback')
            OR pricing_source IS NULL
    ),
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_archived_savings_created_at
    ON archived_savings_snapshots(created_at);
"#;

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    MessageNotFound {
        conversation_id: String,
        message_id: String,
    },
    Validation(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(error) => write!(formatter, "storage I/O error: {error}"),
            StorageError::Sql(error) => write!(formatter, "SQLite error: {error}"),
            StorageError::MessageNotFound {
                conversation_id,
                message_id,
            } => write!(
                formatter,
                "message `{message_id}` was not found in conversation `{conversation_id}`"
            ),
            StorageError::Validation(message) => formatter.write_str(message),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StorageTruncateErrorCode {
    MessageNotFound,
    Storage,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageTruncateError {
    pub code: StorageTruncateErrorCode,
    pub message: String,
}

impl From<StorageError> for StorageTruncateError {
    fn from(error: StorageError) -> Self {
        let code = match &error {
            StorageError::MessageNotFound { .. }
            | StorageError::Sql(rusqlite::Error::QueryReturnedNoRows) => {
                StorageTruncateErrorCode::MessageNotFound
            }
            _ => StorageTruncateErrorCode::Storage,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

pub(crate) type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug)]
pub(crate) struct AgentMemorySessionCandidate {
    pub id: String,
    pub kind: String,
    pub cwd: String,
    pub cli_session_id: String,
    pub created_at: String,
}

pub struct Storage {
    connection: Mutex<Connection>,
    session_started_at: String,
    #[allow(dead_code)]
    path: PathBuf,
    /// The keychain item of the Halv session token (issue #50).
    session_token: session_token::SessionTokenVault,
}

pub(crate) struct AgentSessionRecord<'a> {
    pub id: &'a str,
    pub agent_kind: &'a str,
    pub lane: &'a str,
    pub mode: &'a str,
    pub ponytail_mode: Option<PonytailMode>,
    pub project_id: Option<i64>,
    pub conversation_id: Option<&'a str>,
    pub cwd: &'a str,
    pub cli_session_id: Option<&'a str>,
    pub custom_title: Option<&'a str>,
    pub default_title: &'a str,
    pub generated_title: Option<&'a str>,
    pub ai_vault_title: Option<&'a str>,
    pub ai_vault_session_id: Option<&'a str>,
    pub generation: u64,
    pub status: &'a str,
    pub created_at: &'a str,
    pub last_active_at: &'a str,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSessionRow {
    pub id: String,
    pub agent_kind: String,
    pub lane: String,
    pub mode: String,
    pub ponytail_mode: Option<String>,
    pub project_id: Option<i64>,
    pub conversation_id: Option<String>,
    pub cwd: String,
    pub cli_session_id: Option<String>,
    pub custom_title: Option<String>,
    pub default_title: String,
    pub generated_title: Option<String>,
    pub ai_vault_title: Option<String>,
    pub ai_vault_session_id: Option<String>,
    pub generation: u64,
    pub status: String,
    pub created_at: String,
    pub last_active_at: String,
    pub exit_code: Option<i32>,
    pub rtk_db_path: Option<String>,
    pub crux_ledger_path: Option<String>,
    /// How the session uses the Halv Engine: `direct`, `engine`, `workers`
    /// or `isolated` (see `agent_sessions::HeadroomRoute`). `None` for a row
    /// from 1.0.0 or older.
    pub headroom_route: Option<String>,
}

fn agent_session_from_row(row: &Row<'_>) -> rusqlite::Result<AgentSessionRow> {
    let generation = u64::try_from(row.get::<_, i64>(14)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(14, Type::Integer, Box::new(error))
    })?;
    Ok(AgentSessionRow {
        id: row.get(0)?,
        agent_kind: row.get(1)?,
        lane: row.get(2)?,
        mode: row.get(3)?,
        ponytail_mode: row.get(4)?,
        project_id: row.get(5)?,
        conversation_id: row.get(6)?,
        cwd: row.get(7)?,
        cli_session_id: row.get(8)?,
        custom_title: row.get(9)?,
        default_title: row.get(10)?,
        generated_title: row.get(11)?,
        ai_vault_title: row.get(12)?,
        ai_vault_session_id: row.get(13)?,
        generation,
        status: row.get(15)?,
        created_at: row.get(16)?,
        last_active_at: row.get(17)?,
        exit_code: row.get(18)?,
        rtk_db_path: row.get(19)?,
        crux_ledger_path: row.get(20)?,
        headroom_route: row.get(21)?,
    })
}

pub(crate) struct AgentSessionTurnRecord<'a> {
    pub session_id: &'a str,
    pub turn_index: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub ponytail_mode: Option<PonytailMode>,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: Option<f64>,
    pub savings_tokens: Option<i64>,
    pub savings_usd: Option<f64>,
    pub pricing_source: Option<&'a str>,
    pub crux_tokens_saved: i64,
    pub savings_source: Option<&'a str>,
    pub has_message_snapshot: bool,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CruxIndexState {
    pub project_path: String,
    pub last_indexed_git_head: Option<String>,
    pub last_index_stats: String,
    pub last_index_time: String,
}

#[derive(Debug, Clone)]
pub struct NewMessage<'a> {
    pub conversation_id: &'a str,
    pub role: &'a str,
    pub content: &'a str,
    pub model: Option<&'a str>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub error: bool,
    pub mentions: Option<&'a [String]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageAttachment {
    pub name: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug)]
struct ConversationForkSource {
    title: String,
    mode: String,
    model: String,
    goal: Option<String>,
    project_id: Option<i64>,
    agent_mode: i64,
    approval_policy_override: Option<String>,
    workspace_id: Option<String>,
    workflow_mode: String,
    associated_worktree_id: Option<String>,
    memories_mode: Option<String>,
    fast_mode: Option<i64>,
    agent_backend: Option<String>,
    agent_session_model: Option<String>,
}

#[derive(Debug)]
struct MessageForkSource {
    sequence: i64,
    id: String,
    role: String,
    content: String,
    created_at: String,
    model: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    error: i64,
    mentions: Option<String>,
}

#[derive(Debug)]
struct MessageAttachmentForkSource {
    position: i64,
    name: String,
    media_type: String,
    size_bytes: i64,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub title_source: String,
    pub mode: String,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
    pub draft: String,
    pub draft_only: bool,
    pub archived_at: Option<i64>,
    pub memories_mode: Option<String>,
    pub fast_mode: bool,
    pub goal: Option<String>,
    pub agent_mode: bool,
    pub workflow_mode: String,
    pub credential_profile_id: Option<String>,
    pub agent_backend: Option<String>,
    pub agent_session_model: Option<String>,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    pub project_root_path: Option<String>,
    pub project_trust_level: Option<String>,
    pub approval_policy_override: Option<String>,
    pub message_count: u32,
    /// The bound workspace. Null for a chat without a project.
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProfile {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub kind: String,
    pub priority: i64,
    pub status: String,
    pub last_validated_at: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: Option<i64>,
    pub provider: String,
    pub credential_profile_id: Option<String>,
    pub model: Option<String>,
    pub window_kind: String,
    pub limit_value: Option<i64>,
    pub remaining: Option<i64>,
    pub used: Option<i64>,
    pub resets_at: Option<String>,
    pub retry_after_seconds: Option<i64>,
    pub source: String,
    pub confidence: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCooldown {
    pub provider: String,
    pub credential_profile_id: Option<String>,
    pub reason: String,
    pub next_attempt_at: Option<String>,
    pub last_error: Option<String>,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentMemory {
    pub id: i64,
    pub project_id: Option<i64>,
    pub project_name: Option<String>,
    pub content: String,
    pub source_conversation_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageSearchResult {
    pub conversation_id: String,
    pub conversation_title: String,
    pub message_id: String,
    pub role: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversationExportFormat {
    Markdown,
    Json,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ConversationExport {
    schema: &'static str,
    conversation: ConversationExportMetadata,
    messages: Vec<StoredMessage>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ConversationExportMetadata {
    id: String,
    title: String,
    model: String,
    created_at: String,
    updated_at: String,
    goal: Option<String>,
    agent_mode: bool,
    workflow_mode: String,
    project_id: Option<i64>,
    project_name: Option<String>,
    project_root_path: Option<String>,
    project_trust_level: Option<String>,
    approval_policy_override: Option<String>,
    message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlanItem {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTaskPlan {
    pub conversation_id: String,
    pub explanation: Option<String>,
    pub plan: Vec<TaskPlanItem>,
    pub updated_at: String,
}

/// One routed task brief. The brief text itself never reaches the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRoute {
    pub id: String,
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub title: String,
    pub brief_chars: i64,
    pub complexity: String,
    pub complexity_confidence: f64,
    pub needs_reasoning: f64,
    pub risky: f64,
    pub tier: String,
    pub lane: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub reasons: Vec<String>,
    pub worker_lane: String,
    pub status: String,
    pub escalated_from_id: Option<String>,
    pub provider: String,
    pub provider_model: String,
    pub latency_ms: i64,
    pub created_at: String,
    pub updated_at: String,
    /// The exact model that the worker would run without JEV: the coordinator
    /// model that a worker or a workflow agent inherits.
    #[serde(default)]
    pub default_model: Option<String>,
    /// The coordinator effort that a workflow agent inherits, when known.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Where the default came from: `observed` (the coordinator transcript),
    /// `coordinator` (the routing call), `hint` (the launch), or `none`.
    #[serde(default)]
    pub default_source: Option<String>,
    /// The session ended before a report. Such a route is `failed`, but its
    /// outcome is unknown.
    #[serde(default)]
    pub closed_unreported: bool,
    /// The workflow fields of a `route_tasks` route. `None` on other routes.
    #[serde(default)]
    pub workflow: Option<TaskRouteWorkflow>,
    /// The receipt of a reported workflow route.
    #[serde(default)]
    pub receipt: Option<TaskRouteReceipt>,
}

/// One route of a `route_tasks` batch (ADR 0014).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRouteWorkflow {
    pub batch_id: String,
    /// The short workflow name that the coordinator sent.
    pub name: Option<String>,
    /// The key of the route in `args.jev` of the Workflow script.
    pub key: String,
    /// `read`, `implement`, or `verify`.
    pub role: String,
    /// The number of similar agents that share this route.
    pub instances: i64,
}

/// The outcome and the token use of one reported workflow route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRouteReceipt {
    pub route_id: String,
    pub session_id: String,
    /// `passed`, `failed`, or `escalated` (failed, then retried on the next
    /// route).
    pub outcome: String,
    pub agents: i64,
    pub failed_agents: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    /// `measured` from the agent transcripts, `reported` by the coordinator,
    /// or `none`.
    pub usage_source: String,
    pub reported_at: String,
    /// The exact models of the measured agents. Halv fills this field when it
    /// reads the receipt.
    #[serde(default)]
    pub models: Vec<String>,
}

/// The measured use of one workflow agent on one model, read from its Claude
/// Code transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRouteAgent {
    pub route_id: String,
    pub agent_id: String,
    pub observed_model: String,
    pub workflow_run_id: String,
    pub label: String,
    pub observed_effort: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    /// The list price of the agent's messages, each at its own prompt size
    /// (ADR 0016). `None` in rows of older builds.
    #[serde(default)]
    pub actual_usd: Option<f64>,
    #[serde(default)]
    pub price_version: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationCompaction {
    pub id: i64,
    pub conversation_id: String,
    pub upto_message_id: i64,
    pub summary: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentMessage {
    pub id: String,
    pub agent_id: String,
    pub role: String,
    pub content: String,
    pub generation: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubagentThread {
    pub agent_id: String,
    pub conversation_id: String,
    pub turn_id: String,
    pub task_name: String,
    pub task: String,
    pub status: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<SubagentMessage>,
}

#[derive(Debug, Clone)]
pub struct SubagentThreadUpdate<'a> {
    pub agent_id: &'a str,
    pub conversation_id: &'a str,
    pub turn_id: &'a str,
    pub task_name: &'a str,
    pub task: &'a str,
    pub status: &'a str,
    pub result: Option<&'a str>,
    pub error: Option<&'a str>,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub trust_level: Option<String>,
    pub approval_policy: Option<String>,
    pub created_at: String,
    pub last_opened_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub project_id: i64,
    pub name: String,
    pub kind: String,
    pub root_path: String,
    pub git_common_dir: Option<String>,
    pub base_ref: Option<String>,
    pub base_commit: Option<String>,
    pub branch: Option<String>,
    pub local_environment_config_path: Option<String>,
    pub managed: bool,
    pub pinned: bool,
    pub state: String,
    pub created_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Clone)]
pub struct NewWorkspace<'a> {
    pub project_id: i64,
    pub name: &'a str,
    pub root_path: &'a str,
    pub git_common_dir: Option<&'a str>,
    pub base_ref: Option<&'a str>,
    pub base_commit: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub local_environment_config_path: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct SavingsReportEvent {
    pub id: String,
    pub tokens_saved: i64,
    pub savings_usd: f64,
    /// The optional fields of ADR 0015; absent for old events.
    #[serde(flatten)]
    pub dollars: Option<savings_dollars::ReportDollars>,
    /// UNIX seconds when the app queued the event. The server stores it
    /// as the event time, so a late batch lands on its own days.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
}

/// The most tokens one report event can carry. The server refuses a
/// larger event, so the queue splits a larger sample into parts.
pub(crate) const MAX_TOKENS_SAVED_PER_REPORT_EVENT: i64 = 10_000_000;

/// One part of a queued sample: its queue id, its tokens and its share of
/// the sample dollars.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReportEventPart {
    pub id: String,
    pub tokens_saved: i64,
    pub share: f64,
}

/// Splits a sample into parts of at most MAX_TOKENS_SAVED_PER_REPORT_EVENT
/// tokens. The first part keeps the sample id; the next parts get the
/// suffix `#part2`, `#part3` and so on. A sample within the limit is one
/// part with share 1.
pub(crate) fn report_event_parts(event_id: &str, tokens_saved: i64) -> Vec<ReportEventPart> {
    if tokens_saved <= MAX_TOKENS_SAVED_PER_REPORT_EVENT {
        return vec![ReportEventPart {
            id: event_id.to_string(),
            tokens_saved,
            share: 1.0,
        }];
    }
    let mut parts = Vec::new();
    let mut remaining = tokens_saved;
    while remaining > 0 {
        let part_tokens = remaining.min(MAX_TOKENS_SAVED_PER_REPORT_EVENT);
        let index = parts.len() + 1;
        parts.push(ReportEventPart {
            id: if index == 1 {
                event_id.to_string()
            } else {
                format!("{event_id}#part{index}")
            },
            tokens_saved: part_tokens,
            share: part_tokens as f64 / tokens_saved as f64,
        });
        remaining -= part_tokens;
    }
    parts
}

/// Removes the unreported extra parts of a split sample, so that a smaller
/// sample with the same id does not leave stale parts behind.
pub(super) fn delete_unreported_report_parts(
    connection: &Connection,
    event_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM savings_report_queue
         WHERE instr(message_id, ?1 || '#part') = 1
           AND reported_at IS NULL AND rejected_at IS NULL",
        params![event_id],
    )?;
    Ok(())
}

/// The state of the savings report queue, for support.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsQueueCounts {
    pub unreported: u64,
    pub rejected: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalActivationDiagnostics {
    pub first_activated_at: String,
    pub last_activated_at: String,
    pub launch_count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalDataCounts {
    pub conversations: u64,
    pub messages: u64,
    pub projects: u64,
    pub workspaces: u64,
    pub memories: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalStorageDiagnostics {
    pub healthy: bool,
    pub schema_version: u64,
    pub activation: LocalActivationDiagnostics,
    pub counts: LocalDataCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    pub server_url: String,
    pub token: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub entitlements_json: Option<String>,
    pub synced_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    #[serde(default)]
    pub sequence: i64,
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub model: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    pub tokens_saved: Option<i64>,
    pub savings_usd: Option<f64>,
    pub pricing_source: Option<String>,
    #[serde(default)]
    pub crux_tokens_saved: Option<i64>,
    #[serde(default)]
    pub crux_included_in_total: bool,
    pub cached_tokens: Option<i64>,
    pub savings_source: Option<String>,
    /// The dollar method of the saving (ADR 0015); absent for old rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TurnDiffSummary {
    pub turn_id: String,
    pub diff: String,
    pub cwd: String,
    pub created_at: String,
    pub revertible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSnapshot {
    pub turn_id: String,
    pub display_path: String,
    pub absolute_path: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after_exists: Option<bool>,
    pub after_sha256: Option<String>,
    pub created_at: String,
}

fn checkpoint_from_row(row: &Row<'_>) -> rusqlite::Result<crate::checkpoints::Checkpoint> {
    Ok(crate::checkpoints::Checkpoint {
        id: row.get(0)?,
        session_id: row.get(1)?,
        turn_id: row.get(2)?,
        repo_root: row.get(3)?,
        commit_sha: row.get(4)?,
        tree_sha: row.get(5)?,
        head_sha: row.get(6)?,
        branch: row.get(7)?,
        kind: row.get(8)?,
        label: row.get(9)?,
        created_at: row.get(10)?,
    })
}

impl Storage {
    pub fn open_app_data_dir(app_data_dir: &Path) -> StorageResult<Self> {
        crate::app_data::create_private_dir(app_data_dir)?;
        Self::open(app_data_dir.join(DB_FILENAME))
    }

    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        Self::open_with_session_token_store(path, session_token::default_store())
    }

    pub(crate) fn open_with_session_token_store(
        path: impl AsRef<Path>,
        session_token_store: std::sync::Arc<dyn session_token::SessionTokenStore>,
    ) -> StorageResult<Self> {
        Self::open_with_session_token_vault(
            path,
            session_token::SessionTokenVault::new(session_token_store),
        )
    }

    pub(crate) fn open_with_session_token_vault(
        path: impl AsRef<Path>,
        session_token: session_token::SessionTokenVault,
    ) -> StorageResult<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // The database can hold the Halv session token when the keychain is
        // not available (issue #50). SQLite gives new -wal
        // and -shm files the mode of the database file. Existing files keep
        // the mode that an earlier release gave them.
        crate::app_data::create_private_file(&path)?;
        for suffix in ["-wal", "-shm"] {
            let mut companion = path.clone().into_os_string();
            companion.push(suffix);
            crate::app_data::restrict_file(Path::new(&companion));
        }

        let connection = Connection::open(&path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL lets the read-only savings MCP reader query while the app writes.
        let journal_mode: String =
            connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            eprintln!("storage: journal_mode is `{journal_mode}`, not wal");
        }
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        run_migrations(&connection)?;
        record_local_activation(&connection)?;
        recover_interrupted_subagents(&connection)?;

        let storage = Self {
            connection: Mutex::new(connection),
            session_started_at: now_string(),
            path,
            session_token,
        };
        if let Err(error) = storage.move_session_token_to_keychain() {
            eprintln!("Halv could not move the session token to the keychain: {error}");
        }
        Ok(storage)
    }

    #[cfg(test)]
    pub(crate) fn from_test_connection(connection: Connection, session_started_at: &str) -> Self {
        Self {
            connection: Mutex::new(connection),
            session_started_at: session_started_at.to_string(),
            path: PathBuf::from(":memory:"),
            session_token: session_token::SessionTokenVault::new(session_token::default_store()),
        }
    }

    pub(crate) fn database_path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn insert_session_checkpoint(
        &self,
        checkpoint: &crate::checkpoints::Checkpoint,
    ) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "INSERT INTO session_checkpoints (
                   id, session_id, turn_id, repo_root, commit_sha, tree_sha,
                   head_sha, branch, kind, label, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    checkpoint.id,
                    checkpoint.session_id,
                    checkpoint.turn_id,
                    checkpoint.repo_root,
                    checkpoint.commit_sha,
                    checkpoint.tree_sha,
                    checkpoint.head_sha,
                    checkpoint.branch,
                    checkpoint.kind,
                    checkpoint.label,
                    checkpoint.created_at,
                ],
            )?;
        Ok(())
    }

    pub(crate) fn session_checkpoints(
        &self,
        session_id: &str,
    ) -> StorageResult<Vec<crate::checkpoints::Checkpoint>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, session_id, turn_id, repo_root, commit_sha, tree_sha,
                    head_sha, branch, kind, label, created_at
             FROM session_checkpoints
             WHERE session_id = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let checkpoints = statement
            .query_map(params![session_id], checkpoint_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(checkpoints)
    }

    pub(crate) fn session_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> StorageResult<Option<crate::checkpoints::Checkpoint>> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .query_row(
                "SELECT id, session_id, turn_id, repo_root, commit_sha, tree_sha,
                        head_sha, branch, kind, label, created_at
                 FROM session_checkpoints
                 WHERE id = ?1",
                params![checkpoint_id],
                checkpoint_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub(crate) fn session_checkpoints_beyond(
        &self,
        session_id: &str,
        keep: usize,
    ) -> StorageResult<Vec<crate::checkpoints::Checkpoint>> {
        let keep =
            i64::try_from(keep).map_err(|error| StorageError::Validation(error.to_string()))?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, session_id, turn_id, repo_root, commit_sha, tree_sha,
                    head_sha, branch, kind, label, created_at
             FROM session_checkpoints
             WHERE session_id = ?1
             ORDER BY created_at DESC, id DESC
             LIMIT -1 OFFSET ?2",
        )?;
        let checkpoints = statement
            .query_map(params![session_id, keep], checkpoint_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(checkpoints)
    }

    pub(crate) fn delete_session_checkpoint(&self, checkpoint_id: &str) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "DELETE FROM session_checkpoints WHERE id = ?1",
                params![checkpoint_id],
            )?;
        Ok(())
    }

    pub(crate) fn upsert_agent_session(
        &self,
        session: &AgentSessionRecord<'_>,
    ) -> StorageResult<()> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO agent_sessions (
               id, agent_kind, lane, mode, ponytail_mode, project_id, conversation_id, cwd,
                 cli_session_id, custom_title, status, created_at, last_active_at, exit_code,
                 default_title, generated_title, ai_vault_title, ai_vault_session_id, generation
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
                agent_kind = excluded.agent_kind,
               lane = excluded.lane,
               mode = excluded.mode,
               ponytail_mode = excluded.ponytail_mode,
               project_id = excluded.project_id,
               conversation_id = excluded.conversation_id,
               cwd = excluded.cwd,
                cli_session_id = excluded.cli_session_id,
                custom_title = excluded.custom_title,
                status = excluded.status,
                created_at = excluded.created_at,
                last_active_at = excluded.last_active_at,
                 exit_code = excluded.exit_code,
                 default_title = excluded.default_title,
                 generated_title = excluded.generated_title,
                 ai_vault_title = excluded.ai_vault_title,
                 ai_vault_session_id = excluded.ai_vault_session_id,
                 generation = excluded.generation",
            params![
                session.id,
                session.agent_kind,
                session.lane,
                session.mode,
                session.ponytail_mode.map(PonytailMode::as_str),
                session.project_id,
                session.conversation_id,
                session.cwd,
                session.cli_session_id,
                session.custom_title,
                session.status,
                session.created_at,
                session.last_active_at,
                session.exit_code,
                session.default_title,
                session.generated_title,
                session.ai_vault_title,
                session.ai_vault_session_id,
                i64::try_from(session.generation).unwrap_or(i64::MAX),
            ],
        )?;
        if matches!(session.status, "exited" | "failed") {
            close_session_routes(&transaction, session.id)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn get_agent_session(&self, id: &str) -> StorageResult<Option<AgentSessionRow>> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .query_row(
                "SELECT id, agent_kind, lane, mode, ponytail_mode, project_id,
                        conversation_id, cwd, cli_session_id, custom_title, default_title,
                        generated_title, ai_vault_title, ai_vault_session_id, generation,
                        status, created_at, last_active_at, exit_code, rtk_db_path,
                        crux_ledger_path, headroom_route
                 FROM agent_sessions
                 WHERE id = ?1",
                params![id],
                agent_session_from_row,
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub(crate) fn list_agent_sessions_with_status(
        &self,
        status: &str,
    ) -> StorageResult<Vec<AgentSessionRow>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, agent_kind, lane, mode, ponytail_mode, project_id,
                    conversation_id, cwd, cli_session_id, custom_title, default_title,
                    generated_title, ai_vault_title, ai_vault_session_id, generation,
                    status, created_at, last_active_at, exit_code, rtk_db_path,
                    crux_ledger_path, headroom_route
             FROM agent_sessions
             WHERE status = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = statement
            .query_map(params![status], agent_session_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn list_agent_session_lanes(&self) -> StorageResult<Vec<(String, String)>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, lane
             FROM agent_sessions
             ORDER BY created_at DESC",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub(crate) fn update_agent_session_savings_paths(
        &self,
        id: &str,
        rtk_db_path: Option<&Path>,
        crux_ledger_path: Option<&Path>,
    ) -> StorageResult<()> {
        if rtk_db_path.is_none() && crux_ledger_path.is_none() {
            return Ok(());
        }
        let rtk_db_path = rtk_db_path.map(|path| path.to_string_lossy().into_owned());
        let crux_ledger_path = crux_ledger_path.map(|path| path.to_string_lossy().into_owned());
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE agent_sessions
                 SET rtk_db_path = COALESCE(?2, rtk_db_path),
                     crux_ledger_path = COALESCE(?3, crux_ledger_path)
                 WHERE id = ?1",
                params![id, rtk_db_path, crux_ledger_path],
            )?;
        Ok(())
    }

    /// Saves how an agent session uses the Halv Engine. The next launch
    /// reads it for the restored sessions.
    pub(crate) fn set_agent_session_headroom_route(
        &self,
        id: &str,
        headroom_route: &str,
    ) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE agent_sessions SET headroom_route = ?2 WHERE id = ?1",
                params![id, headroom_route],
            )?;
        Ok(())
    }

    // The supervisor preserves live sessions across app restarts.
    pub(crate) fn reconcile_session_routes(
        &self,
        live_ids: &std::collections::HashSet<String>,
    ) -> StorageResult<()> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT id FROM agent_sessions
                 WHERE status IN ('starting', 'running', 'waiting_input', 'exited', 'failed')",
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for id in ids {
            if live_ids.contains(&id) {
                continue;
            }
            transaction.execute(
                "UPDATE agent_sessions SET status = 'exited', exit_code = NULL
                 WHERE id = ?1 AND status IN ('starting', 'running', 'waiting_input')",
                params![id],
            )?;
            close_session_routes(&transaction, &id)?;
        }
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn insert_agent_session_turn(
        &self,
        turn: &AgentSessionTurnRecord<'_>,
    ) -> StorageResult<i64> {
        self.insert_priced_agent_session_turn(turn, None)
    }

    /// Inserts a turn. `dollars` carries the dollar method of ADR 0015.
    pub(crate) fn insert_priced_agent_session_turn(
        &self,
        turn: &AgentSessionTurnRecord<'_>,
        dollars: Option<&savings_dollars::TurnDollars<'_>>,
    ) -> StorageResult<i64> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            // The SQL guard protects against stale in-memory counters while
            // the resumed-session migration initializes the normal counter.
            "INSERT INTO agent_session_turns (
               session_id, turn_index, input_tokens, output_tokens, ponytail_mode,
                cache_read_tokens, cache_creation_tokens, cost_usd,
                savings_tokens, savings_usd, pricing_source, crux_tokens_saved,
                savings_source, has_message_snapshot, created_at, occurred_local_date,
                model, usd_method, price_version, prompt_rate_usd_per_m, source_kind,
                cache_write_1h_tokens
            ) VALUES (
                ?1,
                MAX(
                    ?2,
                    (SELECT COALESCE(MAX(turn_index), -1) + 1
                     FROM agent_session_turns WHERE session_id = ?1)
                ),
                ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                date(?15, 'localtime'), ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                turn.session_id,
                turn.turn_index,
                turn.input_tokens,
                turn.output_tokens,
                turn.ponytail_mode.map(PonytailMode::as_str),
                turn.cache_read_tokens,
                turn.cache_creation_tokens,
                turn.cost_usd,
                turn.savings_tokens,
                turn.savings_usd,
                turn.pricing_source,
                turn.crux_tokens_saved.max(0),
                turn.savings_source,
                if turn.has_message_snapshot { 1 } else { 0 },
                turn.created_at,
                dollars.and_then(|dollars| dollars.model),
                dollars.and_then(|dollars| dollars.usd_method),
                dollars.and_then(|dollars| dollars.price_version),
                dollars.and_then(|dollars| dollars.prompt_rate_usd_per_m),
                dollars.and_then(|dollars| dollars.source_kind),
                dollars.map_or(0, |dollars| dollars.cache_write_1h_tokens.max(0)),
            ],
        )?;
        Ok(connection.last_insert_rowid())
    }

    pub(crate) fn upsert_agent_message_usage(
        &self,
        message_id: &str,
        cached_tokens: i64,
        savings_source: &str,
    ) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "INSERT INTO agent_message_usage (message_id, cached_tokens, savings_source)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(message_id) DO UPDATE SET
                   cached_tokens = excluded.cached_tokens,
                   savings_source = excluded.savings_source",
                params![message_id, cached_tokens.max(0), savings_source],
            )?;
        Ok(())
    }

    pub(crate) fn local_diagnostics(&self) -> StorageResult<LocalStorageDiagnostics> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let quick_check: String =
            connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        let schema_version =
            connection.query_row("SELECT COUNT(*) FROM migrations", [], |row| {
                row.get::<_, u64>(0)
            })?;
        let activation = connection.query_row(
            "SELECT first_activated_at, last_activated_at, launch_count
             FROM local_diagnostics WHERE id = 1",
            [],
            |row| {
                Ok(LocalActivationDiagnostics {
                    first_activated_at: row.get(0)?,
                    last_activated_at: row.get(1)?,
                    launch_count: row.get(2)?,
                })
            },
        )?;
        let count = |table: &str| -> StorageResult<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            connection
                .query_row(&sql, [], |row| row.get(0))
                .map_err(StorageError::from)
        };
        let counts = LocalDataCounts {
            conversations: count("conversations")?,
            messages: count("messages")?,
            projects: count("projects")?,
            workspaces: count("workspaces")?,
            memories: count("agent_memories")?,
        };

        Ok(LocalStorageDiagnostics {
            healthy: quick_check == "ok",
            schema_version,
            activation,
            counts,
        })
    }

    pub(crate) fn free_turns_used(&self) -> StorageResult<u64> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT free_turns_used FROM local_diagnostics WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    pub(crate) fn first_launch_at(&self) -> StorageResult<String> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT first_launch_at FROM local_diagnostics WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(StorageError::from)
    }

    #[allow(dead_code)]
    pub fn ensure_conversation(
        &self,
        conversation_id: Option<String>,
        first_message: &str,
        model: &str,
        project_id: Option<i64>,
    ) -> StorageResult<String> {
        self.ensure_conversation_in_workspace(
            conversation_id,
            first_message,
            model,
            project_id,
            None,
        )
    }

    pub fn ensure_conversation_in_workspace(
        &self,
        conversation_id: Option<String>,
        first_message: &str,
        model: &str,
        project_id: Option<i64>,
        workspace_id: Option<&str>,
    ) -> StorageResult<String> {
        let id = conversation_id.unwrap_or_else(new_uuid_v4);
        let title = title_from_message(first_message);
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");

        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![&id],
            |row| row.get(0),
        )?;

        if !exists {
            let workspace_id =
                resolve_workspace_for_new_conversation(&connection, project_id, workspace_id)?;
            connection.execute(
                "INSERT INTO conversations
                    (id, title, model, created_at, updated_at, project_id,
                     workspace_id, associated_worktree_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6,
                         CASE WHEN (SELECT kind FROM workspaces WHERE id = ?6) = 'worktree'
                              THEN ?6 ELSE NULL END)",
                params![&id, title, model, now, project_id, workspace_id],
            )?;
        }

        Ok(id)
    }

    pub fn claim_title_generation(&self, conversation_id: &str) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET title_generation_attempted = 1
             WHERE id = ?1
               AND title_source = 'default'
               AND title_generation_attempted = 0",
            params![conversation_id],
        )?;
        Ok(updated == 1)
    }

    pub fn set_generated_conversation_title(
        &self,
        conversation_id: &str,
        title: &str,
    ) -> StorageResult<Option<ConversationSummary>> {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.trim().is_empty() {
            return Ok(None);
        }
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET title = ?1,
                 title_source = 'generated'
             WHERE id = ?2
               AND title_source = 'default'
               AND title_generation_attempted = 1",
            params![title, conversation_id],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        conversation_summary(&connection, conversation_id)
            .optional()
            .map_err(Into::into)
    }

    pub fn set_conversation_title(
        &self,
        conversation_id: &str,
        title: &str,
    ) -> StorageResult<ConversationSummary> {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() || title.chars().count() > 120 {
            return Err(StorageError::Validation(
                "conversation title must contain 1 to 120 characters".to_string(),
            ));
        }
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET title = ?1,
                 title_source = 'manual',
                 title_generation_attempted = 1
             WHERE id = ?2",
            params![title, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn set_conversation_pinned(
        &self,
        conversation_id: &str,
        pinned: bool,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET pinned = ?1 WHERE id = ?2",
            params![i64::from(pinned), conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn set_conversation_draft(
        &self,
        conversation_id: &str,
        draft: &str,
        draft_only: bool,
    ) -> StorageResult<Option<ConversationSummary>> {
        if draft.chars().count() > MAX_CONVERSATION_DRAFT_CHARS {
            return Err(StorageError::Validation(format!(
                "conversation draft exceeds {MAX_CONVERSATION_DRAFT_CHARS} characters"
            )));
        }
        let draft = if draft.trim().is_empty() { "" } else { draft };
        let title = title_from_message(draft);
        let now = now_string();
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let (stored_draft_only, message_count, goal): (i64, i64, Option<String>) = transaction
            .query_row(
                "SELECT draft_only,
                        (SELECT COUNT(*) FROM messages WHERE conversation_id = conversations.id),
                        goal
                 FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        if draft.is_empty() && stored_draft_only != 0 && message_count == 0 && goal.is_none() {
            transaction.execute(
                "DELETE FROM conversations WHERE id = ?1",
                params![conversation_id],
            )?;
            transaction.commit()?;
            return Ok(None);
        }
        transaction.execute(
            "UPDATE conversations
             SET draft = ?1,
                 draft_only = CASE WHEN ?2 = 1 THEN 1 ELSE draft_only END,
                 title = CASE
                   WHEN (?2 = 1 OR draft_only = 1)
                    AND title_source = 'default'
                    AND ?1 != '' THEN ?3
                   ELSE title
                 END,
                 updated_at = ?4
             WHERE id = ?5",
            params![draft, i64::from(draft_only), title, now, conversation_id],
        )?;
        let summary = conversation_summary(&transaction, conversation_id)?;
        transaction.commit()?;
        Ok(Some(summary))
    }

    pub fn consume_conversation_draft(
        &self,
        conversation_id: &str,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET draft = '', draft_only = 0, updated_at = ?1
             WHERE id = ?2",
            params![now_string(), conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn touch_conversation(&self, conversation_id: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now_string(), conversation_id],
        )?;
        Ok(())
    }

    pub fn set_conversation_archived(
        &self,
        conversation_id: &str,
        archived: bool,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let archived_at = archived.then(|| Utc::now().timestamp());
        let updated = connection.execute(
            "UPDATE conversations SET archived_at = ?1 WHERE id = ?2",
            params![archived_at, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn set_conversation_memories_mode(
        &self,
        conversation_id: &str,
        mode: Option<&str>,
    ) -> StorageResult<ConversationSummary> {
        validate_memories_mode(mode)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET memories_mode = ?1 WHERE id = ?2",
            params![mode, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_memories_mode(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT memories_mode FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn set_conversation_fast_mode(
        &self,
        conversation_id: &str,
        enabled: bool,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET fast_mode = ?1 WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_fast_mode(&self, conversation_id: &str) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let value = connection.query_row(
            "SELECT fast_mode FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        Ok(value.unwrap_or_default() != 0)
    }

    pub fn conversation_project_id(&self, conversation_id: &str) -> StorageResult<Option<i64>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT project_id FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn set_conversation_mode(
        &self,
        conversation_id: &str,
        mode: &str,
    ) -> StorageResult<ConversationSummary> {
        if !matches!(mode, "chat" | "agent") {
            return Err(StorageError::Validation(
                "conversation mode must be `chat` or `agent`".to_string(),
            ));
        }
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET mode = ?1 WHERE id = ?2",
            params![mode, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_agent_backend(
        &self,
        conversation_id: &str,
    ) -> StorageResult<(Option<String>, Option<String>)> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT agent_backend, agent_session_model FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
    }

    pub fn set_conversation_agent_backend(
        &self,
        conversation_id: &str,
        agent_backend: Option<&str>,
        agent_session_model: Option<&str>,
    ) -> StorageResult<ConversationSummary> {
        let backend = agent_backend
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if backend.is_some_and(|value| {
            !matches!(
                value,
                "claude_session"
                    | "codex_session"
                    | "kimi_session"
                    | "zai_session"
                    | "grok_session"
            )
        }) {
            return Err(StorageError::Validation(
                "Unsupported conversation agent backend".to_string(),
            ));
        }
        let model = if backend.is_some() {
            Some(
                agent_session_model
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        StorageError::Validation(
                            "Claude session conversations need a model".to_string(),
                        )
                    })?,
            )
        } else {
            None
        };
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET agent_backend = ?1, agent_session_model = ?2 WHERE id = ?3",
            params![backend, model, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn list_credential_profiles(&self) -> StorageResult<Vec<CredentialProfile>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, provider, name, kind, priority, status,
                    last_validated_at, metadata_json, created_at, updated_at
             FROM credential_profiles
             ORDER BY provider ASC, priority ASC, name ASC",
        )?;
        let rows = statement.query_map([], credential_profile_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_credential_profile(&self, id: &str) -> StorageResult<Option<CredentialProfile>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT id, provider, name, kind, priority, status,
                        last_validated_at, metadata_json, created_at, updated_at
                 FROM credential_profiles
                 WHERE id = ?1",
                params![id],
                credential_profile_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_credential_profile(&self, profile: &CredentialProfile) -> StorageResult<()> {
        validate_credential_profile_status(&profile.status)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO credential_profiles
                (id, provider, name, kind, priority, status, last_validated_at,
                 metadata_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &profile.id,
                &profile.provider,
                &profile.name,
                &profile.kind,
                profile.priority,
                &profile.status,
                &profile.last_validated_at,
                &profile.metadata_json,
                &profile.created_at,
                &profile.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn rename_credential_profile(
        &self,
        id: &str,
        name: &str,
    ) -> StorageResult<CredentialProfile> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE credential_profiles SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        credential_profile_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_credential_profile_status(
        &self,
        id: &str,
        status: &str,
    ) -> StorageResult<CredentialProfile> {
        validate_credential_profile_status(status)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE credential_profiles SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        credential_profile_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_credential_profile_priority(
        &self,
        id: &str,
        priority: i64,
    ) -> StorageResult<CredentialProfile> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE credential_profiles SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            params![priority, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        credential_profile_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_credential_profile_validated(
        &self,
        id: &str,
        last_validated_at: Option<&str>,
        metadata_json: Option<&str>,
    ) -> StorageResult<CredentialProfile> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE credential_profiles
             SET last_validated_at = ?1, metadata_json = ?2, updated_at = ?3
             WHERE id = ?4",
            params![last_validated_at, metadata_json, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        credential_profile_by_id(&connection, id).map_err(Into::into)
    }

    pub fn delete_credential_profile(&self, id: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute("DELETE FROM credential_profiles WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn upsert_usage_window(&self, record: &UsageWindow) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO usage_windows
                (provider, credential_profile_id, model, window_kind,
                 limit_value, remaining, used, resets_at, retry_after_seconds,
                 source, confidence, captured_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT DO UPDATE SET
                limit_value = excluded.limit_value,
                remaining = excluded.remaining,
                used = excluded.used,
                resets_at = excluded.resets_at,
                retry_after_seconds = excluded.retry_after_seconds,
                source = excluded.source,
                confidence = excluded.confidence,
                captured_at = excluded.captured_at",
            params![
                &record.provider,
                &record.credential_profile_id,
                &record.model,
                &record.window_kind,
                record.limit_value,
                record.remaining,
                record.used,
                &record.resets_at,
                record.retry_after_seconds,
                &record.source,
                &record.confidence,
                &record.captured_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_usage_windows(&self) -> StorageResult<Vec<UsageWindow>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, provider, credential_profile_id, model, window_kind,
                    limit_value, remaining, used, resets_at, retry_after_seconds,
                    source, confidence, captured_at
             FROM usage_windows
             ORDER BY provider ASC, credential_profile_id ASC, model ASC, window_kind ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(UsageWindow {
                id: Some(row.get(0)?),
                provider: row.get(1)?,
                credential_profile_id: row.get(2)?,
                model: row.get(3)?,
                window_kind: row.get(4)?,
                limit_value: row.get(5)?,
                remaining: row.get(6)?,
                used: row.get(7)?,
                resets_at: row.get(8)?,
                retry_after_seconds: row.get(9)?,
                source: row.get(10)?,
                confidence: row.get(11)?,
                captured_at: row.get(12)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_profile_cooldown(&self, record: &ProfileCooldown) -> StorageResult<()> {
        let last_error = record
            .last_error
            .as_deref()
            .map(|message| message.chars().take(300).collect::<String>());
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO profile_cooldowns
                (provider, credential_profile_id, reason, next_attempt_at,
                 last_error, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO UPDATE SET
                reason = excluded.reason,
                next_attempt_at = excluded.next_attempt_at,
                last_error = excluded.last_error,
                recorded_at = excluded.recorded_at",
            params![
                &record.provider,
                &record.credential_profile_id,
                &record.reason,
                &record.next_attempt_at,
                last_error,
                &record.recorded_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_profile_cooldowns(&self) -> StorageResult<Vec<ProfileCooldown>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT provider, credential_profile_id, reason, next_attempt_at,
                    last_error, recorded_at
             FROM profile_cooldowns
             ORDER BY provider ASC, credential_profile_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(ProfileCooldown {
                provider: row.get(0)?,
                credential_profile_id: row.get(1)?,
                reason: row.get(2)?,
                next_attempt_at: row.get(3)?,
                last_error: row.get(4)?,
                recorded_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn clear_profile_cooldown(
        &self,
        provider: &str,
        credential_profile_id: Option<&str>,
    ) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "DELETE FROM profile_cooldowns
             WHERE provider = ?1 AND credential_profile_id IS ?2",
            params![provider, credential_profile_id],
        )?;
        Ok(())
    }

    pub fn set_conversation_credential_profile(
        &self,
        conversation_id: &str,
        profile_id: Option<String>,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations SET credential_profile_id = ?1 WHERE id = ?2",
            params![profile_id, conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_credential_profile(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT credential_profile_id FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub(crate) fn agent_memory_subscription_session(
        &self,
        session_id: &str,
    ) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_sessions WHERE id = ?1 AND ((agent_kind = 'codex' AND lane = 'codex_chatgpt') OR (agent_kind = 'claude' AND lane = 'claude_subscription')))",
            params![session_id],
            |row| row.get(0),
        )?)
    }

    pub fn agent_memory_turn_processed(&self, source_key: &str) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute_batch("CREATE TABLE IF NOT EXISTS agent_memory_processed_turns (source_key TEXT PRIMARY KEY NOT NULL)")?;
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_memory_processed_turns WHERE source_key = ?1)",
            params![source_key],
            |row| row.get(0),
        )?)
    }

    pub fn mark_agent_memory_turn_processed(&self, source_key: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute_batch("CREATE TABLE IF NOT EXISTS agent_memory_processed_turns (source_key TEXT PRIMARY KEY NOT NULL)")?;
        connection.execute(
            "INSERT OR IGNORE INTO agent_memory_processed_turns(source_key) VALUES (?1)",
            params![source_key],
        )?;
        Ok(())
    }

    pub fn conversation_id_for_agent_session(
        &self,
        session_id: &str,
    ) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        Ok(connection
            .query_row(
                "SELECT conversation_id FROM agent_sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub(crate) fn recent_memory_agent_sessions(
        &self,
    ) -> StorageResult<Vec<AgentMemorySessionCandidate>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare("SELECT id, agent_kind, cwd, cli_session_id, created_at FROM agent_sessions WHERE mode = 'terminal_embedded' AND cli_session_id IS NOT NULL AND ((agent_kind = 'codex' AND lane = 'codex_chatgpt') OR (agent_kind = 'claude' AND lane = 'claude_subscription')) ORDER BY last_active_at DESC LIMIT 32")?;
        let rows = statement
            .query_map([], |row| {
                Ok(AgentMemorySessionCandidate {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    cwd: row.get(2)?,
                    cli_session_id: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_agent_memories(&self) -> StorageResult<Vec<AgentMemory>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT am.id, am.project_id, p.name, am.content,
                    am.source_conversation_id, am.created_at, am.updated_at
             FROM agent_memories am
             LEFT JOIN projects p ON p.id = am.project_id
             ORDER BY am.updated_at DESC, am.id DESC",
        )?;
        let rows = statement.query_map([], agent_memory_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn agent_memories_for_scope(
        &self,
        project_id: Option<i64>,
        limit: usize,
    ) -> StorageResult<Vec<AgentMemory>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT am.id, am.project_id, p.name, am.content,
                    am.source_conversation_id, am.created_at, am.updated_at
             FROM agent_memories am
             LEFT JOIN projects p ON p.id = am.project_id
             WHERE (?1 IS NULL AND am.project_id IS NULL)
                OR (?1 IS NOT NULL AND (am.project_id IS NULL OR am.project_id = ?1))
             ORDER BY am.updated_at DESC, am.id DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![project_id, limit as i64], agent_memory_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn insert_agent_memories(
        &self,
        project_id: Option<i64>,
        contents: &[String],
        source_conversation_id: Option<&str>,
    ) -> StorageResult<usize> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let now = Utc::now().timestamp();
        let mut inserted = 0;
        for content in contents {
            let content = content.trim();
            if content.is_empty() {
                continue;
            }
            if content.chars().count() > 2_000 {
                return Err(StorageError::Validation(
                    "agent memory content must be at most 2000 characters".to_string(),
                ));
            }
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM agent_memories
                    WHERE project_id IS ?1
                      AND lower(trim(content)) = lower(?2)
                 )",
                params![project_id, content],
                |row| row.get(0),
            )?;
            if exists {
                continue;
            }
            transaction.execute(
                "INSERT INTO agent_memories
                    (project_id, content, source_conversation_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![project_id, content, source_conversation_id, now],
            )?;
            inserted += 1;
        }

        let count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM agent_memories WHERE project_id IS ?1",
            params![project_id],
            |row| row.get(0),
        )?;
        let excess = count.saturating_sub(MAX_AGENT_MEMORIES_PER_SCOPE as i64);
        if excess > 0 {
            transaction.execute(
                "DELETE FROM agent_memories
                 WHERE id IN (
                    SELECT id
                    FROM agent_memories
                    WHERE project_id IS ?1
                    ORDER BY updated_at ASC, id ASC
                    LIMIT ?2
                 )",
                params![project_id, excess],
            )?;
        }
        transaction.commit()?;
        Ok(inserted)
    }

    pub fn delete_agent_memory(&self, id: i64) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .execute("DELETE FROM agent_memories WHERE id = ?1", params![id])
            .map(|deleted| deleted == 1)
            .map_err(Into::into)
    }

    pub fn clear_agent_memories(&self) -> StorageResult<usize> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .execute("DELETE FROM agent_memories", [])
            .map_err(Into::into)
    }

    pub fn fork_conversation(
        &self,
        source_id: &str,
        upto_sequence: i64,
    ) -> StorageResult<ConversationSummary> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let source = transaction.query_row(
            "SELECT title, model, goal, project_id, agent_mode,
                    approval_policy_override, workspace_id, workflow_mode,
                    associated_worktree_id, memories_mode, fast_mode,
                    agent_backend, agent_session_model, mode
             FROM conversations
             WHERE id = ?1",
            params![source_id],
            |row| {
                Ok(ConversationForkSource {
                    title: row.get(0)?,
                    model: row.get(1)?,
                    goal: row.get(2)?,
                    project_id: row.get(3)?,
                    agent_mode: row.get(4)?,
                    approval_policy_override: row.get(5)?,
                    workspace_id: row.get(6)?,
                    workflow_mode: row.get(7)?,
                    associated_worktree_id: row.get(8)?,
                    memories_mode: row.get(9)?,
                    fast_mode: row.get(10)?,
                    agent_backend: row.get(11)?,
                    agent_session_model: row.get(12)?,
                    mode: row.get(13)?,
                })
            },
        )?;
        let fork_point_exists: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM messages
                WHERE conversation_id = ?1 AND rowid = ?2
             )",
            params![source_id, upto_sequence],
            |row| row.get(0),
        )?;
        if !fork_point_exists {
            return Err(StorageError::MessageNotFound {
                conversation_id: source_id.to_string(),
                message_id: upto_sequence.to_string(),
            });
        }

        let fork_id = new_uuid_v4();
        let fork_title = format!("Fork of {}", source.title);
        let now = now_string();
        transaction.execute(
            "INSERT INTO conversations
                (id, title, model, created_at, updated_at, goal, project_id,
                 agent_mode, approval_policy_override, title_source,
                 title_generation_attempted, workspace_id, workflow_mode,
                 associated_worktree_id, memories_mode, fast_mode,
                 agent_backend, agent_session_model, mode)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7, ?8, 'manual', 1, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                fork_id,
                fork_title,
                source.model,
                now,
                source.goal,
                source.project_id,
                source.agent_mode,
                source.approval_policy_override,
                source.workspace_id,
                source.workflow_mode,
                source.associated_worktree_id,
                source.memories_mode,
                source.fast_mode,
                source.agent_backend,
                source.agent_session_model,
                source.mode,
            ],
        )?;

        let source_messages = {
            let mut statement = transaction.prepare(
                "SELECT rowid, id, role, content, created_at, model,
                        input_tokens, output_tokens, error, mentions
                 FROM messages
                 WHERE conversation_id = ?1 AND rowid <= ?2
                 ORDER BY rowid ASC",
            )?;
            let rows = statement.query_map(params![source_id, upto_sequence], |row| {
                Ok(MessageForkSource {
                    sequence: row.get(0)?,
                    id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                    model: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    error: row.get(8)?,
                    mentions: row.get(9)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut source_attachments = {
            let mut statement = transaction.prepare(
                "SELECT a.message_id, a.position, a.name, a.media_type,
                        a.size_bytes, a.data
                 FROM message_attachments a
                 INNER JOIN messages m ON m.id = a.message_id
                 WHERE m.conversation_id = ?1 AND m.rowid <= ?2
                 ORDER BY m.rowid ASC, a.position ASC, a.id ASC",
            )?;
            let rows = statement.query_map(params![source_id, upto_sequence], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    MessageAttachmentForkSource {
                        position: row.get(1)?,
                        name: row.get(2)?,
                        media_type: row.get(3)?,
                        size_bytes: row.get(4)?,
                        data: row.get(5)?,
                    },
                ))
            })?;
            let mut attachments = HashMap::<String, Vec<MessageAttachmentForkSource>>::new();
            for row in rows {
                let (message_id, attachment) = row?;
                attachments.entry(message_id).or_default().push(attachment);
            }
            attachments
        };

        let mut sequence_mapping = HashMap::<i64, i64>::new();
        for message in source_messages {
            let new_message_id = new_uuid_v4();
            transaction.execute(
                "INSERT INTO messages
                    (id, conversation_id, role, content, created_at, model,
                     input_tokens, output_tokens, error, mentions)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    new_message_id,
                    fork_id,
                    message.role,
                    message.content,
                    message.created_at,
                    message.model,
                    message.input_tokens,
                    message.output_tokens,
                    message.error,
                    message.mentions,
                ],
            )?;
            let new_sequence = transaction.last_insert_rowid();
            sequence_mapping.insert(message.sequence, new_sequence);

            for attachment in source_attachments.remove(&message.id).unwrap_or_default() {
                transaction.execute(
                    "INSERT INTO message_attachments
                        (id, message_id, position, name, media_type, size_bytes, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        new_uuid_v4(),
                        new_message_id,
                        attachment.position,
                        attachment.name,
                        attachment.media_type,
                        attachment.size_bytes,
                        attachment.data,
                    ],
                )?;
            }
        }

        let latest_compaction = transaction
            .query_row(
                "SELECT upto_message_id, summary, created_at
                 FROM conversation_compactions
                 WHERE conversation_id = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![source_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((source_watermark, summary, created_at)) =
            latest_compaction.filter(|(watermark, _, _)| *watermark <= upto_sequence)
        {
            let new_watermark = sequence_mapping.get(&source_watermark).ok_or_else(|| {
                StorageError::Validation(format!(
                    "conversation compaction watermark {source_watermark} has no source message"
                ))
            })?;
            transaction.execute(
                "INSERT INTO conversation_compactions
                    (conversation_id, upto_message_id, summary, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![fork_id, new_watermark, summary, created_at],
            )?;
        }

        let summary = conversation_summary(&transaction, &fork_id)?;
        transaction.commit()?;
        Ok(summary)
    }

    pub fn insert_message(&self, message: NewMessage<'_>) -> StorageResult<StoredMessage> {
        self.insert_message_with_attachments_and_ponytail(message, &[], None)
    }

    pub(crate) fn insert_message_with_ponytail(
        &self,
        message: NewMessage<'_>,
        ponytail_mode: Option<PonytailMode>,
    ) -> StorageResult<StoredMessage> {
        self.insert_message_with_attachments_and_ponytail(message, &[], ponytail_mode)
    }

    pub fn insert_message_with_attachments(
        &self,
        message: NewMessage<'_>,
        attachments: &[NewMessageAttachment],
    ) -> StorageResult<StoredMessage> {
        self.insert_message_with_attachments_and_ponytail(message, attachments, None)
    }

    fn insert_message_with_attachments_and_ponytail(
        &self,
        message: NewMessage<'_>,
        attachments: &[NewMessageAttachment],
        ponytail_mode: Option<PonytailMode>,
    ) -> StorageResult<StoredMessage> {
        let id = new_uuid_v4();
        let created_at = now_string();
        let error = if message.error { 1 } else { 0 };
        let mentions_json = encode_mentions(message.mentions)?;
        let stored_attachments = attachments
            .iter()
            .map(|attachment| MessageAttachment {
                id: new_uuid_v4(),
                name: attachment.name.clone(),
                media_type: attachment.media_type.clone(),
                size_bytes: attachment.data.len() as u64,
                data: STANDARD.encode(&attachment.data),
            })
            .collect::<Vec<_>>();
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;

        transaction.execute(
            "INSERT INTO messages
                (id, conversation_id, role, content, created_at, model, input_tokens, output_tokens, error, mentions, ponytail_mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                message.conversation_id,
                message.role,
                message.content,
                created_at,
                message.model,
                message.input_tokens,
                message.output_tokens,
                error,
                mentions_json,
                ponytail_mode.map(PonytailMode::as_str),
            ],
        )?;
        let sequence = transaction.last_insert_rowid();
        for (position, (stored, attachment)) in
            stored_attachments.iter().zip(attachments).enumerate()
        {
            transaction.execute(
                "INSERT INTO message_attachments
                    (id, message_id, position, name, media_type, size_bytes, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    stored.id,
                    id,
                    position as i64,
                    stored.name,
                    stored.media_type,
                    stored.size_bytes as i64,
                    attachment.data,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE conversations
             SET updated_at = ?1,
                 model = COALESCE(?2, model),
                 draft = '',
                 draft_only = 0
             WHERE id = ?3",
            params![created_at, message.model, message.conversation_id],
        )?;
        transaction.commit()?;

        Ok(StoredMessage {
            sequence,
            id,
            conversation_id: message.conversation_id.to_string(),
            role: message.role.to_string(),
            content: message.content.to_string(),
            created_at,
            model: message.model.map(ToString::to_string),
            input_tokens: message.input_tokens,
            output_tokens: message.output_tokens,
            error: message.error,
            mentions: message.mentions.map(|mentions| mentions.to_vec()),
            attachments: stored_attachments,
            tokens_saved: None,
            savings_usd: None,
            pricing_source: None,
            crux_tokens_saved: None,
            crux_included_in_total: false,
            cached_tokens: None,
            savings_source: None,
            usd_method: None,
        })
    }

    #[cfg(test)]
    pub fn insert_savings_snapshot(
        &self,
        message_id: &str,
        tokens_saved: Option<i64>,
        savings_usd: Option<f64>,
    ) -> StorageResult<()> {
        self.insert_savings_snapshot_with_crux(
            message_id,
            tokens_saved,
            savings_usd,
            savings_usd.map(|_| PricingSource::Headroom),
            0,
        )
    }

    pub fn insert_savings_snapshot_with_crux(
        &self,
        message_id: &str,
        tokens_saved: Option<i64>,
        savings_usd: Option<f64>,
        pricing_source: Option<PricingSource>,
        crux_tokens_saved: i64,
    ) -> StorageResult<()> {
        self.insert_savings_snapshot_inner(
            message_id,
            tokens_saved,
            savings_usd,
            pricing_source,
            crux_tokens_saved,
            None,
        )
    }

    /// Inserts a snapshot that the caller priced with a method of ADR 0015.
    /// `tokens_saved` excludes crux tokens; `dollars.usd` includes them.
    pub(crate) fn insert_priced_savings_snapshot(
        &self,
        message_id: &str,
        tokens_saved: i64,
        crux_tokens_saved: i64,
        dollars: &savings_dollars::SnapshotDollars<'_>,
    ) -> StorageResult<()> {
        self.insert_savings_snapshot_inner(
            message_id,
            Some(tokens_saved),
            Some(dollars.usd),
            Some(dollars.source),
            crux_tokens_saved,
            Some(dollars),
        )
    }

    fn insert_savings_snapshot_inner(
        &self,
        message_id: &str,
        tokens_saved: Option<i64>,
        savings_usd: Option<f64>,
        pricing_source: Option<PricingSource>,
        crux_tokens_saved: i64,
        dollars: Option<&savings_dollars::SnapshotDollars<'_>>,
    ) -> StorageResult<()> {
        let crux_tokens_saved = crux_tokens_saved.max(0);
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let (agent_kind, model) = message_pricing_context(&connection, message_id)?;
        let input_price = pricing::input_price(agent_kind.as_deref(), model.as_deref());
        let mut pricing_source =
            pricing_source.or_else(|| savings_usd.map(|_| PricingSource::Headroom));
        let mut savings_usd = savings_usd;
        if savings_usd.is_none() && tokens_saved.is_some_and(|tokens| tokens > 0) {
            savings_usd = Some(input_price.savings_usd(tokens_saved.unwrap_or_default()));
            pricing_source = Some(input_price.source);
        }
        if crux_tokens_saved > 0 && dollars.is_none() {
            savings_usd =
                Some(savings_usd.unwrap_or_default() + input_price.savings_usd(crux_tokens_saved));
            pricing_source = Some(
                pricing_source
                    .map(|source| source.combine(input_price.source))
                    .unwrap_or(input_price.source),
            );
        }
        let tokens_saved = if crux_tokens_saved > 0 {
            Some(
                tokens_saved
                    .unwrap_or_default()
                    .saturating_add(crux_tokens_saved),
            )
        } else {
            tokens_saved
        };
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO savings_snapshots
                (message_id, tokens_saved, savings_usd, pricing_source, crux_tokens_saved,
                 occurred_local_date, usd_method, price_version)
             VALUES (?1, ?2, ?3, ?4, ?5,
                     (SELECT date(created_at, 'localtime') FROM messages WHERE id = ?1),
                     ?6, ?7)
             ON CONFLICT(message_id) DO UPDATE SET
                tokens_saved = excluded.tokens_saved,
                savings_usd = excluded.savings_usd,
                pricing_source = excluded.pricing_source,
                crux_tokens_saved = excluded.crux_tokens_saved,
                occurred_local_date = excluded.occurred_local_date,
                usd_method = excluded.usd_method,
                price_version = excluded.price_version",
            params![
                message_id,
                tokens_saved,
                savings_usd,
                pricing_source.map(PricingSource::as_str),
                crux_tokens_saved,
                dollars.map(|dollars| dollars.usd_method),
                dollars.map(|dollars| dollars.price_version),
            ],
        )?;
        if let Some(tokens_saved) = tokens_saved.filter(|tokens| *tokens > 0) {
            let parts = report_event_parts(message_id, tokens_saved);
            if parts.len() > 1 {
                delete_unreported_report_parts(&transaction, message_id)?;
            }
            for part in parts {
                let part_usd = savings_usd.map(|usd| usd * part.share);
                transaction.execute(
                    "INSERT INTO savings_report_queue
                        (message_id, tokens_saved, savings_usd, created_at, reported_at,
                         kind, net_usd, method, price_version)
                     VALUES (?1, ?2, ?3, ?4, NULL, 'context', ?5, ?6, ?7)
                     ON CONFLICT(message_id) DO UPDATE SET
                        tokens_saved = excluded.tokens_saved,
                        savings_usd = excluded.savings_usd,
                        net_usd = excluded.net_usd,
                        method = excluded.method,
                        price_version = excluded.price_version
                     WHERE savings_report_queue.reported_at IS NULL",
                    params![
                        part.id,
                        part.tokens_saved,
                        part_usd,
                        now_string(),
                        dollars.map(|_| part_usd),
                        dollars.map(|dollars| dollars.usd_method),
                        dollars.map(|dollars| dollars.price_version),
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn enqueue_savings_report(
        &self,
        event_id: &str,
        tokens_saved: i64,
        savings_usd: f64,
    ) -> StorageResult<()> {
        if tokens_saved <= 0 {
            return Ok(());
        }
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let parts = report_event_parts(event_id, tokens_saved);
        if parts.len() > 1 {
            delete_unreported_report_parts(&transaction, event_id)?;
        }
        for part in parts {
            transaction.execute(
                "INSERT INTO savings_report_queue
                    (message_id, tokens_saved, savings_usd, created_at, reported_at)
                 VALUES (?1, ?2, ?3, ?4, NULL)
                 ON CONFLICT(message_id) DO UPDATE SET
                    tokens_saved = excluded.tokens_saved,
                    savings_usd = excluded.savings_usd
                 WHERE savings_report_queue.reported_at IS NULL",
                params![
                    part.id,
                    part.tokens_saved,
                    savings_usd * part.share,
                    now_string()
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn unreported_savings(
        &self,
        limit: usize,
    ) -> StorageResult<Vec<SavingsReportEvent>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT message_id, tokens_saved, savings_usd, kind, net_usd, method,
                    price_version, created_at
             FROM savings_report_queue
             WHERE reported_at IS NULL AND rejected_at IS NULL
             ORDER BY created_at ASC, message_id ASC
             LIMIT ?1",
        )?;
        let limit = i64::try_from(limit.min(500)).unwrap_or(500);
        let events = statement
            .query_map(params![limit], |row| {
                let savings_usd: f64 = row.get(2)?;
                let method: Option<String> = row.get(5)?;
                let created_at = row
                    .get::<_, String>(7)
                    .ok()
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
                    .map(|value| value.timestamp());
                let dollars = match method {
                    Some(method) => Some(savings_dollars::ReportDollars {
                        kind: row.get(3)?,
                        net_usd: row.get::<_, Option<f64>>(4)?.unwrap_or(savings_usd),
                        method,
                        price_version: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    }),
                    None => None,
                };
                Ok(SavingsReportEvent {
                    id: row.get(0)?,
                    tokens_saved: row.get(1)?,
                    savings_usd,
                    dollars,
                    created_at,
                })
            })?
            .collect::<Result<Vec<_>, rusqlite::Error>>()?;
        Ok(events)
    }

    pub(crate) fn mark_savings_reported(&self, event_ids: &[String]) -> StorageResult<()> {
        if event_ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let reported_at = now_string();
        for event_id in event_ids {
            transaction.execute(
                "UPDATE savings_report_queue
                 SET reported_at = ?1
                 WHERE message_id = ?2 AND reported_at IS NULL",
                params![&reported_at, event_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The rows of the report queue that wait for the server, and the
    /// dead letters the server refused.
    pub(crate) fn savings_queue_counts(&self) -> StorageResult<SavingsQueueCounts> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT
                    COALESCE(SUM(reported_at IS NULL AND rejected_at IS NULL), 0),
                    COALESCE(SUM(rejected_at IS NOT NULL), 0)
                 FROM savings_report_queue",
                [],
                |row| {
                    Ok(SavingsQueueCounts {
                        unreported: row.get(0)?,
                        rejected: row.get(1)?,
                    })
                },
            )
            .map_err(StorageError::from)
    }

    pub(crate) fn crux_index_state(
        &self,
        project_path: &str,
    ) -> StorageResult<Option<CruxIndexState>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT project_path, last_indexed_git_head, last_index_stats, last_index_time
                 FROM crux_index_state
                 WHERE project_path = ?1",
                params![project_path],
                |row| {
                    Ok(CruxIndexState {
                        project_path: row.get(0)?,
                        last_indexed_git_head: row.get(1)?,
                        last_index_stats: row.get(2)?,
                        last_index_time: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    pub(crate) fn upsert_crux_index_state(&self, state: &CruxIndexState) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO crux_index_state (
                project_path, last_indexed_git_head, last_index_stats, last_index_time
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_path) DO UPDATE SET
                last_indexed_git_head = excluded.last_indexed_git_head,
                last_index_stats = excluded.last_index_stats,
                last_index_time = excluded.last_index_time",
            params![
                state.project_path,
                state.last_indexed_git_head,
                state.last_index_stats,
                state.last_index_time,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_conversation_goal(
        &self,
        conversation_id: Option<String>,
        goal: Option<&str>,
        model: Option<&str>,
        project_id: Option<i64>,
    ) -> StorageResult<ConversationSummary> {
        self.set_conversation_goal_in_workspace(conversation_id, goal, model, project_id, None)
    }

    pub fn set_conversation_goal_in_workspace(
        &self,
        conversation_id: Option<String>,
        goal: Option<&str>,
        model: Option<&str>,
        project_id: Option<i64>,
        workspace_id: Option<&str>,
    ) -> StorageResult<ConversationSummary> {
        let id = conversation_id.unwrap_or_else(new_uuid_v4);
        let goal = normalize_goal(goal);
        // Keep the raw Option for the UPDATE so COALESCE preserves the
        // conversation's existing model; default only when inserting.
        let model = model.map(str::trim).filter(|value| !value.is_empty());
        let insert_model = model.unwrap_or(DEFAULT_STORAGE_MODEL);
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");

        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![&id],
            |row| row.get(0),
        )?;

        if !exists {
            let workspace_id =
                resolve_workspace_for_new_conversation(&connection, project_id, workspace_id)?;
            let title = goal
                .as_deref()
                .map(title_from_goal)
                .unwrap_or_else(|| "New conversation".to_string());
            connection.execute(
                "INSERT INTO conversations
                    (id, title, model, created_at, updated_at, project_id,
                     workspace_id, associated_worktree_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6,
                         CASE WHEN (SELECT kind FROM workspaces WHERE id = ?6) = 'worktree'
                              THEN ?6 ELSE NULL END)",
                params![&id, title, insert_model, now, project_id, workspace_id],
            )?;
        }

        connection.execute(
            "UPDATE conversations
             SET goal = ?1, updated_at = ?2, model = COALESCE(?3, model)
             WHERE id = ?4",
            params![goal, now, model, &id],
        )?;

        conversation_summary(&connection, &id).map_err(Into::into)
    }

    pub fn conversation_goal(&self, conversation_id: &str) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT goal FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
    }

    pub fn conversation_agent_mode(&self, conversation_id: &str) -> StorageResult<bool> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT agent_mode FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|value| value.unwrap_or_default() != 0)
            .map_err(Into::into)
    }

    pub fn set_conversation_agent_mode(
        &self,
        conversation_id: &str,
        enabled: bool,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        if enabled {
            require_conversation_project(&connection, conversation_id)?;
        }
        let updated = connection.execute(
            "UPDATE conversations
             SET agent_mode = ?1, updated_at = ?2
             WHERE id = ?3",
            params![if enabled { 1 } else { 0 }, now_string(), conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_workflow_mode(&self, conversation_id: &str) -> StorageResult<String> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT workflow_mode FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::Sql(rusqlite::Error::QueryReturnedNoRows))
    }

    pub fn set_conversation_workflow_mode(
        &self,
        conversation_id: &str,
        mode: &str,
    ) -> StorageResult<ConversationSummary> {
        validate_workflow_mode(mode)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET workflow_mode = ?1, updated_at = ?2
             WHERE id = ?3",
            params![mode, now_string(), conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_task_plan(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<ConversationTaskPlan>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT conversation_id, explanation, plan_json, updated_at
                 FROM conversation_task_plans
                 WHERE conversation_id = ?1",
                params![conversation_id],
                |row| {
                    let plan_json: String = row.get(2)?;
                    let plan =
                        serde_json::from_str::<Vec<TaskPlanItem>>(&plan_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(ConversationTaskPlan {
                        conversation_id: row.get(0)?,
                        explanation: row.get(1)?,
                        plan,
                        updated_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn upsert_conversation_task_plan(
        &self,
        conversation_id: &str,
        explanation: Option<&str>,
        plan: &[TaskPlanItem],
    ) -> StorageResult<ConversationTaskPlan> {
        let explanation = explanation.map(str::trim).filter(|value| !value.is_empty());
        let plan_json = serde_json::to_string(plan).map_err(|error| {
            StorageError::Validation(format!("failed to encode conversation task plan: {error}"))
        })?;
        let updated_at = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO conversation_task_plans
                (conversation_id, explanation, plan_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(conversation_id) DO UPDATE SET
                explanation = excluded.explanation,
                plan_json = excluded.plan_json,
                updated_at = excluded.updated_at",
            params![conversation_id, explanation, plan_json, updated_at],
        )?;
        Ok(ConversationTaskPlan {
            conversation_id: conversation_id.to_string(),
            explanation: explanation.map(ToString::to_string),
            plan: plan.to_vec(),
            updated_at,
        })
    }

    // The MCP process writes routes through its own connection. The app keeps
    // these methods for the task board and for tests.
    #[allow(dead_code)]
    pub fn insert_task_route(&self, route: &TaskRoute) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        insert_task_route_row(&connection, route).map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn update_task_route_status(&self, id: &str, status: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = update_task_route_status_row(&connection, id, status)?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    pub fn list_task_routes_for_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Vec<TaskRoute>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        list_task_routes_rows(&connection, "conversation_id = ?1", conversation_id)
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn list_task_routes_for_session(&self, session_id: &str) -> StorageResult<Vec<TaskRoute>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        list_task_routes_rows(&connection, "session_id = ?1", session_id).map_err(Into::into)
    }

    pub fn insert_conversation_compaction(
        &self,
        conversation_id: &str,
        upto_message_id: i64,
        summary: &str,
    ) -> StorageResult<ConversationCompaction> {
        let created_at = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO conversation_compactions
                (conversation_id, upto_message_id, summary, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, upto_message_id, summary, created_at],
        )?;
        Ok(ConversationCompaction {
            id: connection.last_insert_rowid(),
            conversation_id: conversation_id.to_string(),
            upto_message_id,
            summary: summary.to_string(),
            created_at,
        })
    }

    pub fn latest_conversation_compaction(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<ConversationCompaction>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT id, conversation_id, upto_message_id, summary, created_at
                 FROM conversation_compactions
                 WHERE conversation_id = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![conversation_id],
                |row| {
                    Ok(ConversationCompaction {
                        id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        upto_message_id: row.get(2)?,
                        summary: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn upsert_subagent_thread(&self, thread: SubagentThreadUpdate<'_>) -> StorageResult<()> {
        validate_subagent_status(thread.status)?;
        if thread.generation == 0 {
            return Err(StorageError::Validation(
                "subagent generation must be positive".to_string(),
            ));
        }
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO subagent_threads
                (agent_id, conversation_id, turn_id, task_name, task, status,
                 result, error, generation, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(agent_id) DO UPDATE SET
                turn_id = excluded.turn_id,
                task_name = excluded.task_name,
                task = excluded.task,
                status = excluded.status,
                result = excluded.result,
                error = excluded.error,
                generation = excluded.generation,
                updated_at = excluded.updated_at",
            params![
                thread.agent_id,
                thread.conversation_id,
                thread.turn_id,
                thread.task_name,
                thread.task,
                thread.status,
                thread.result,
                thread.error,
                thread.generation as i64,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn append_subagent_message(
        &self,
        agent_id: &str,
        role: &str,
        content: &str,
        generation: u64,
    ) -> StorageResult<SubagentMessage> {
        validate_subagent_message_role(role)?;
        if generation == 0 {
            return Err(StorageError::Validation(
                "subagent generation must be positive".to_string(),
            ));
        }
        let message = SubagentMessage {
            id: new_uuid_v4(),
            agent_id: agent_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            generation,
            created_at: now_string(),
        };
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO subagent_messages
                (id, agent_id, role, content, generation, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &message.id,
                &message.agent_id,
                &message.role,
                &message.content,
                message.generation as i64,
                &message.created_at,
            ],
        )?;
        Ok(message)
    }

    pub fn list_subagent_threads(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Vec<SubagentThread>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT agent_id, conversation_id, turn_id, task_name, task, status,
                    result, error, generation, created_at, updated_at
             FROM subagent_threads
             WHERE conversation_id = ?1
             ORDER BY created_at ASC, agent_id ASC",
        )?;
        let rows = statement.query_map(params![conversation_id], |row| {
            let generation: i64 = row.get(8)?;
            Ok(SubagentThread {
                agent_id: row.get(0)?,
                conversation_id: row.get(1)?,
                turn_id: row.get(2)?,
                task_name: row.get(3)?,
                task: row.get(4)?,
                status: row.get(5)?,
                result: row.get(6)?,
                error: row.get(7)?,
                generation: generation.max(1) as u64,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                messages: Vec::new(),
            })
        })?;
        let mut threads = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut message_statement = connection.prepare(
            "SELECT id, agent_id, role, content, generation, created_at
             FROM subagent_messages
             WHERE agent_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        for thread in &mut threads {
            let rows = message_statement.query_map(params![&thread.agent_id], |row| {
                let generation: i64 = row.get(4)?;
                Ok(SubagentMessage {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    generation: generation.max(1) as u64,
                    created_at: row.get(5)?,
                })
            })?;
            thread.messages = rows.collect::<Result<Vec<_>, _>>()?;
        }
        Ok(threads)
    }

    pub fn total_savings(&self) -> StorageResult<SavingsTotals> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let fallback_usd_per_token = savings_usd_for_tokens(1);
        let lifetime = savings_sum(
            &connection,
            &savings::lifetime_savings_sql(),
            params![fallback_usd_per_token],
        )?;
        let session = savings_sum(
            &connection,
            &savings::session_savings_sql(),
            params![fallback_usd_per_token, &self.session_started_at],
        )?;
        let dollars_since: Option<String> = connection
            .query_row(savings::dollars_since_sql(), [], |row| row.get(0))
            .optional()?;
        let crux_tokens_saved = connection.query_row(
            &savings::lifetime_crux_savings_sql(),
            params![fallback_usd_per_token],
            |row| row.get(0),
        )?;
        let (pending_tokens_saved, pending_savings_usd, pending_dollars_usd) = connection
            .query_row(savings::pending_savings_sql(), [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;

        Ok(SavingsTotals {
            tokens_saved: lifetime.tokens_saved,
            savings_usd: lifetime.savings_usd,
            dollars_usd: lifetime.dollars_usd,
            legacy_tokens_saved: lifetime.legacy_tokens_saved,
            dollars_since,
            session_dollars_usd: session.dollars_usd,
            pending_tokens_saved,
            pending_savings_usd,
            pending_dollars_usd,
            price_version: crate::pricing::current().price_version.clone(),
            pricing_source: lifetime.pricing_source,
            session_tokens_saved: session.tokens_saved,
            session_savings_usd: session.savings_usd,
            session_pricing_source: session.pricing_source,
            crux_tokens_saved,
            crux_included_in_total: true,
            routing: routing_savings::routing_totals(
                &connection,
                routing_savings::RoutingScope::All,
            )?,
            routing_calibration_version: crate::pricing::calibration::current()
                .calibration_version
                .clone(),
        })
    }

    /// Moves persisted turns and returns the next index in one transaction.
    /// A resumed terminal continues the saved session under a new id.
    pub fn migrate_agent_session_turns(
        &self,
        from_session_id: &str,
        to_session_id: &str,
    ) -> StorageResult<usize> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE agent_session_turns SET session_id = ?2 WHERE session_id = ?1",
            params![from_session_id, to_session_id],
        )?;
        let next_index = transaction.query_row(
            "SELECT COALESCE(MAX(turn_index), -1) + 1
             FROM agent_session_turns WHERE session_id = ?1",
            [to_session_id],
            |row| row.get::<_, i64>(0),
        )?;
        let next_index = usize::try_from(next_index).map_err(|_| {
            StorageError::Validation(format!(
                "session `{to_session_id}` has an invalid next turn index"
            ))
        })?;
        transaction.commit()?;
        Ok(next_index)
    }

    /// Per-session savings aggregates exclude GUI turns covered by message
    /// snapshots. The result seeds the live meter after an app restart.
    pub fn agent_session_turn_totals(&self) -> StorageResult<Vec<AgentSessionTurnTotalsRow>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(&savings::agent_session_turn_totals_sql())?;
        let rows = statement
            .query_map(params![savings_usd_for_tokens(1)], |row| {
                Ok(AgentSessionTurnTotalsRow {
                    session_id: row.get::<_, String>(0)?,
                    conversation_id: row.get::<_, Option<String>>(1)?,
                    savings_tokens: row.get::<_, i64>(2)?.max(0) as u64,
                    crux_tokens_saved: row.get::<_, i64>(3)?.max(0) as u64,
                    cache_read_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                    pricing_source: row.get(5)?,
                    dollars_usd: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn savings_since(&self, since_iso: &str) -> StorageResult<SavingsToday> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let fallback_usd_per_token = savings_usd_for_tokens(1);
        let sum = savings_sum(
            &connection,
            &savings::savings_since_sql(),
            params![fallback_usd_per_token, since_iso],
        )?;
        let (recent_savings, recent_dollars) = recent_savings(
            &connection,
            &savings::recent_savings_since_sql(),
            params![fallback_usd_per_token, since_iso],
        )?;

        Ok(SavingsToday {
            tokens_saved: sum.tokens_saved,
            savings_usd: sum.savings_usd,
            pricing_source: sum.pricing_source,
            recent_savings,
            dollars_usd: sum.dollars_usd,
            recent_dollars,
            routing: routing_savings::routing_totals(
                &connection,
                routing_savings::RoutingScope::Since(since_iso),
            )?,
        })
    }

    pub fn savings_today(&self) -> StorageResult<SavingsToday> {
        let local_date = Local::now().format("%Y-%m-%d").to_string();
        self.savings_on_local_date(&local_date)
    }

    pub(crate) fn savings_on_local_date(&self, local_date: &str) -> StorageResult<SavingsToday> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let fallback_usd_per_token = savings_usd_for_tokens(1);
        let sum = savings_sum(
            &connection,
            &savings::savings_on_local_date_sql(),
            params![fallback_usd_per_token, local_date],
        )?;
        let (recent_savings, recent_dollars) = recent_savings(
            &connection,
            &savings::recent_savings_on_local_date_sql(),
            params![fallback_usd_per_token, local_date],
        )?;

        Ok(SavingsToday {
            tokens_saved: sum.tokens_saved,
            savings_usd: sum.savings_usd,
            pricing_source: sum.pricing_source,
            recent_savings,
            dollars_usd: sum.dollars_usd,
            recent_dollars,
            routing: routing_savings::routing_totals(
                &connection,
                routing_savings::RoutingScope::LocalDate(local_date),
            )?,
        })
    }

    pub fn daily_token_activity(&self, since_iso: &str) -> StorageResult<Vec<DailyTokenActivity>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(&savings::daily_token_activity_sql())?;
        let rows = statement
            .query_map(params![savings_usd_for_tokens(1), since_iso], |row| {
                Ok(DailyTokenActivity {
                    date: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    tokens_saved: row.get(3)?,
                    dollars_usd: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The local usage of one UTC day for the daily usage summary (ADR
    /// 0022). The day is `start <= time < end`, in RFC 3339 times.
    pub(crate) fn utc_day_usage(
        &self,
        start: &str,
        end: &str,
    ) -> StorageResult<savings::UtcDayUsage> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let (tokens_saved, dollars_usd, crux_tokens_saved) = connection.query_row(
            &savings::utc_day_savings_sql(),
            params![savings_usd_for_tokens(1), start, end],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let agent_kinds = connection
            .prepare(savings::utc_day_agent_sessions_sql())?
            .query_map(params![start, end], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let routed_tasks = connection.query_row(
            savings::utc_day_routed_tasks_sql(),
            params![start, end],
            |row| row.get(0),
        )?;
        Ok(savings::UtcDayUsage {
            agent_kinds,
            tokens_saved,
            dollars_usd,
            crux_tokens_saved,
            routed_tasks,
        })
    }

    pub fn list_conversations(&self) -> StorageResult<Vec<ConversationSummary>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT
                c.id,
                c.title,
                c.model,
                c.created_at,
                c.updated_at,
                c.archived_at,
                c.memories_mode,
                c.fast_mode,
             c.goal,
             c.agent_mode,
             c.workflow_mode,
             c.credential_profile_id,
             c.project_id,
                p.name AS project_name,
                COALESCE(w.root_path, p.root_path) AS project_root_path,
                p.trust_level AS project_trust_level,
                    c.approval_policy_override,
                    c.agent_backend,
                    c.agent_session_model,
                    COUNT(m.id) AS message_count,
            c.mode,
            c.title_source,
            c.pinned,
            c.draft,
            c.draft_only,
            c.workspace_id
             FROM conversations c
             LEFT JOIN projects p ON p.id = c.project_id
             LEFT JOIN workspaces w ON w.id = c.workspace_id
             LEFT JOIN messages m ON m.conversation_id = c.id
             GROUP BY c.id
             ORDER BY (c.archived_at IS NOT NULL) ASC,
                      c.pinned DESC,
                      c.updated_at DESC,
                      c.created_at DESC",
        )?;

        let rows = statement.query_map([], |row| {
            let fast_mode: Option<i64> = row.get(7)?;
            let agent_mode: i64 = row.get(9)?;
            let message_count: i64 = row.get(19)?;
            Ok(ConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                title_source: row.get(21)?,
                mode: row.get(20)?,
                model: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                pinned: row.get::<_, i64>(22)? != 0,
                draft: row.get(23)?,
                draft_only: row.get::<_, i64>(24)? != 0,
                archived_at: row.get(5)?,
                memories_mode: row.get(6)?,
                fast_mode: fast_mode.unwrap_or_default() != 0,
                goal: row.get(8)?,
                agent_mode: agent_mode != 0,
                workflow_mode: row.get(10)?,
                credential_profile_id: row.get(11)?,
                project_id: row.get(12)?,
                project_name: row.get(13)?,
                project_root_path: row.get(14)?,
                project_trust_level: row.get(15)?,
                approval_policy_override: row.get(16)?,
                agent_backend: row.get(17)?,
                agent_session_model: row.get(18)?,
                message_count: message_count.max(0) as u32,
                workspace_id: row.get(25)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_messages(&self, conversation_id: &str) -> StorageResult<Vec<StoredMessage>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT
                m.rowid,
                m.id,
                m.conversation_id,
                m.role,
                m.content,
                m.created_at,
                m.model,
                m.input_tokens,
                m.output_tokens,
                m.error,
                m.mentions,
               s.tokens_saved,
               s.savings_usd,
               u.cached_tokens,
               u.savings_source,
               s.pricing_source,
               s.crux_tokens_saved,
               s.usd_method
             FROM messages m
             LEFT JOIN savings_snapshots s ON s.message_id = m.id
             LEFT JOIN agent_message_usage u ON u.message_id = m.id
             WHERE m.conversation_id = ?1
             ORDER BY m.rowid ASC",
        )?;

        let rows = statement.query_map(params![conversation_id], |row| {
            let error: i64 = row.get(9)?;
            let mentions = decode_mentions(row.get(10)?, 10)?;
            let crux_tokens_saved: Option<i64> = row.get(16)?;
            Ok(StoredMessage {
                sequence: row.get(0)?,
                id: row.get(1)?,
                conversation_id: row.get(2)?,
                role: row.get(3)?,
                content: row.get(4)?,
                created_at: row.get(5)?,
                model: row.get(6)?,
                input_tokens: row.get(7)?,
                output_tokens: row.get(8)?,
                error: error != 0,
                mentions,
                attachments: Vec::new(),
                tokens_saved: row.get(11)?,
                savings_usd: row.get(12)?,
                cached_tokens: row.get(13)?,
                savings_source: row.get(14)?,
                pricing_source: row.get(15)?,
                crux_tokens_saved,
                crux_included_in_total: crux_tokens_saved.is_some(),
                usd_method: row.get(17)?,
            })
        })?;

        let mut messages = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut attachments = message_attachments_for_conversation(&connection, conversation_id)?;
        for message in &mut messages {
            message.attachments = attachments.remove(&message.id).unwrap_or_default();
        }
        Ok(messages)
    }

    pub fn search_messages(&self, query: &str) -> StorageResult<Vec<MessageSearchResult>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connection.lock().expect("storage lock poisoned");
        if message_fts_table_exists(&connection)? {
            let Some(match_query) = fts_match_query(query) else {
                return Ok(Vec::new());
            };
            search_messages_fts(&connection, &match_query)
        } else {
            search_messages_like(&connection, query)
        }
    }

    pub fn export_conversation(
        &self,
        conversation_id: &str,
        format: ConversationExportFormat,
    ) -> StorageResult<String> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let summary = conversation_summary(&connection, conversation_id)?;
        drop(connection);
        let messages = self.get_messages(conversation_id)?;

        match format {
            ConversationExportFormat::Markdown => {
                Ok(render_conversation_markdown(&summary, &messages))
            }
            ConversationExportFormat::Json => {
                let export = ConversationExport {
                    schema: "halv.conversation.export.v1",
                    conversation: ConversationExportMetadata::from(summary),
                    messages,
                };
                serde_json::to_string_pretty(&export).map_err(|error| {
                    StorageError::Validation(format!("failed to encode export JSON: {error}"))
                })
            }
        }
    }

    pub fn conversation_summary(
        &self,
        conversation_id: &str,
    ) -> StorageResult<ConversationSummary> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn delete_messages_from(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> StorageResult<usize> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let (anchor_created_at, anchor_id): (String, String) = transaction
            .query_row(
                "SELECT created_at, id
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| StorageError::MessageNotFound {
                conversation_id: conversation_id.to_string(),
                message_id: message_id.to_string(),
            })?;
        let message_ids = message_ids_from_anchor(
            &transaction,
            conversation_id,
            &anchor_created_at,
            &anchor_id,
        )?;
        delete_message_dependents(&transaction, conversation_id, &message_ids)?;
        let deleted = transaction.execute(
            "DELETE FROM messages
             WHERE conversation_id = ?1
               AND (created_at, id) >= (?2, ?3)",
            params![conversation_id, anchor_created_at, anchor_id],
        )?;
        refresh_conversation_after_message_delete(&transaction, conversation_id)?;
        transaction.commit()?;
        Ok(deleted)
    }

    pub fn delete_trailing_assistant(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<String>> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let mut statement = transaction.prepare(
            "SELECT id, role, content
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = statement.query_map(params![conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut assistant_ids = Vec::new();
        let mut user_content = None;
        for row in rows {
            let (id, role, content) = row?;
            if role == "assistant" {
                assistant_ids.push(id);
                continue;
            }
            if role == "user" {
                user_content = Some(content);
            }
            break;
        }
        drop(statement);

        if assistant_ids.is_empty() || user_content.is_none() {
            transaction.commit()?;
            return Ok(None);
        }

        delete_message_dependents(&transaction, conversation_id, &assistant_ids)?;
        for id in &assistant_ids {
            transaction.execute(
                "DELETE FROM messages WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id, id],
            )?;
        }
        refresh_conversation_after_message_delete(&transaction, conversation_id)?;
        transaction.commit()?;
        Ok(user_content)
    }

    #[cfg(test)]
    pub fn insert_turn_snapshot(
        &self,
        conversation_id: &str,
        turn_id: &str,
        display_path: &str,
        absolute_path: &Path,
        before: Option<&[u8]>,
    ) -> StorageResult<()> {
        let created_at = now_string();
        let absolute_path = absolute_path.to_string_lossy().to_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT OR IGNORE INTO turn_snapshots
                (conversation_id, turn_id, display_path, absolute_path, before, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                conversation_id,
                turn_id,
                display_path,
                absolute_path,
                before,
                created_at
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_turn_snapshot_with_postcondition(
        &self,
        conversation_id: &str,
        turn_id: &str,
        display_path: &str,
        absolute_path: &Path,
        before: Option<&[u8]>,
        after_exists: Option<bool>,
        after_sha256: Option<&str>,
    ) -> StorageResult<()> {
        let valid_postcondition = matches!(
            (after_exists, after_sha256),
            (Some(false), None) | (Some(true), Some(_))
        ) && after_sha256.is_none_or(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if !valid_postcondition {
            return Err(StorageError::Validation(
                "turn snapshot postcondition is incomplete or invalid".to_string(),
            ));
        }

        let created_at = now_string();
        let absolute_path = absolute_path.to_string_lossy().to_string();
        let after_exists = after_exists.map(i64::from);
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO turn_snapshots
                (conversation_id, turn_id, display_path, absolute_path, before,
                 after_exists, after_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(conversation_id, turn_id, absolute_path) DO UPDATE SET
                after_exists = excluded.after_exists,
                after_sha256 = excluded.after_sha256",
            params![
                conversation_id,
                turn_id,
                display_path,
                absolute_path,
                before,
                after_exists,
                after_sha256,
                created_at
            ],
        )?;
        Ok(())
    }

    pub fn upsert_turn_diff(
        &self,
        conversation_id: &str,
        turn_id: &str,
        cwd: &Path,
        diff: &str,
        revertible: bool,
    ) -> StorageResult<()> {
        let created_at = now_string();
        let cwd = cwd.to_string_lossy().to_string();
        let revertible = if revertible { 1 } else { 0 };
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO turn_diffs (conversation_id, turn_id, diff, cwd, created_at, revertible)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(conversation_id, turn_id) DO UPDATE SET
                diff = turn_diffs.diff || excluded.diff,
                cwd = turn_diffs.cwd,
                revertible = CASE
                    WHEN turn_diffs.revertible = 1 AND excluded.revertible = 1 THEN 1
                    ELSE 0
                END",
            params![conversation_id, turn_id, diff, cwd, created_at, revertible],
        )?;
        Ok(())
    }

    pub fn list_turn_diffs(&self, conversation_id: &str) -> StorageResult<Vec<TurnDiffSummary>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT turn_id, diff, cwd, created_at, revertible
             FROM turn_diffs
             WHERE conversation_id = ?1
             ORDER BY created_at ASC, turn_id ASC",
        )?;
        let rows = statement.query_map(params![conversation_id], |row| {
            let revertible: i64 = row.get(4)?;
            Ok(TurnDiffSummary {
                turn_id: row.get(0)?,
                diff: row.get(1)?,
                cwd: row.get(2)?,
                created_at: row.get(3)?,
                revertible: revertible != 0,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_turn_snapshots(
        &self,
        conversation_id: &str,
        turn_id: &str,
    ) -> StorageResult<Vec<TurnSnapshot>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT turn_id, display_path, absolute_path, before,
                    after_exists, after_sha256, created_at
             FROM turn_snapshots
             WHERE conversation_id = ?1 AND turn_id = ?2
             ORDER BY absolute_path ASC",
        )?;
        let rows = statement.query_map(params![conversation_id, turn_id], |row| {
            let absolute_path: String = row.get(2)?;
            let after_exists: Option<i64> = row.get(4)?;
            Ok(TurnSnapshot {
                turn_id: row.get(0)?,
                display_path: row.get(1)?,
                absolute_path: PathBuf::from(absolute_path),
                before: row.get(3)?,
                after_exists: after_exists.map(|value| value != 0),
                after_sha256: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn delete_turn(&self, conversation_id: &str, turn_id: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "DELETE FROM turn_snapshots WHERE conversation_id = ?1 AND turn_id = ?2",
            params![conversation_id, turn_id],
        )?;
        connection.execute(
            "DELETE FROM turn_diffs WHERE conversation_id = ?1 AND turn_id = ?2",
            params![conversation_id, turn_id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO archived_savings_snapshots
                (message_id, message_role, tokens_saved, savings_usd, pricing_source,
                 created_at, crux_tokens_saved, occurred_local_date, usd_method,
                 price_version)
             SELECT s.message_id, m.role, s.tokens_saved, s.savings_usd,
                    s.pricing_source, m.created_at, s.crux_tokens_saved,
                    s.occurred_local_date, s.usd_method, s.price_version
             FROM savings_snapshots s
             INNER JOIN messages m ON m.id = s.message_id
             WHERE m.conversation_id = ?1",
            params![conversation_id],
        )?;
        transaction.execute(
            "DELETE FROM turn_snapshots WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        transaction.execute(
            "DELETE FROM turn_diffs WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        transaction.execute(
            "DELETE FROM conversation_compactions WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        transaction.execute(
            "DELETE FROM conversations WHERE id = ?1",
            params![conversation_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_project(&self, name: &str, root_path: &str) -> StorageResult<Project> {
        let name = normalize_project_name(name, root_path);
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT OR IGNORE INTO projects (name, root_path, created_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?3)",
            params![name, root_path, now],
        )?;
        connection.execute(
            "UPDATE projects
             SET name = ?1, last_opened_at = ?2
             WHERE root_path = ?3",
            params![name, now, root_path],
        )?;
        let project = project_by_root_path(&connection, root_path)?;
        ensure_primary_workspace(&connection, &project)?;
        Ok(project)
    }

    pub fn list_projects(&self) -> StorageResult<Vec<Project>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let mut statement = connection.prepare(
            "SELECT id, name, root_path, trust_level, approval_policy, created_at, last_opened_at
             FROM projects
             ORDER BY COALESCE(last_opened_at, created_at) DESC, created_at DESC, id DESC",
        )?;
        let rows = statement.query_map([], project_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn open_project(&self, id: i64) -> StorageResult<Project> {
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE projects SET last_opened_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        project_by_id(&connection, id).map_err(Into::into)
    }

    pub fn get_project(&self, id: i64) -> StorageResult<Project> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        project_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_project_trust(&self, id: i64, trust_level: &str) -> StorageResult<Project> {
        validate_trust_level(trust_level)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE projects SET trust_level = ?1 WHERE id = ?2",
            params![trust_level, id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        project_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_project_approval_policy(&self, id: i64, policy: &str) -> StorageResult<Project> {
        validate_approval_policy(policy)?;
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE projects SET approval_policy = ?1 WHERE id = ?2",
            params![policy, id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        project_by_id(&connection, id).map_err(Into::into)
    }

    pub fn list_workspaces(&self, project_id: i64) -> StorageResult<Vec<Workspace>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let project = project_by_id(&connection, project_id)?;
        ensure_primary_workspace(&connection, &project)?;
        let mut statement = connection.prepare(
            "SELECT id, project_id, name, kind, root_path, git_common_dir,
                    base_ref, base_commit, branch, local_environment_config_path,
                    managed, pinned, state,
                    created_at, last_used_at
             FROM workspaces
             WHERE project_id = ?1
             ORDER BY kind = 'primary' DESC, pinned DESC, last_used_at DESC, created_at DESC",
        )?;
        let rows = statement.query_map(params![project_id], workspace_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_workspace(&self, id: &str) -> StorageResult<Workspace> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        workspace_by_id(&connection, id).map_err(Into::into)
    }

    pub fn workspace_for_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<Workspace>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT w.id, w.project_id, w.name, w.kind, w.root_path,
                        w.git_common_dir, w.base_ref, w.base_commit, w.branch,
                        w.local_environment_config_path, w.managed, w.pinned,
                        w.state, w.created_at, w.last_used_at
                 FROM conversations c
                 INNER JOIN workspaces w ON w.id = c.workspace_id
                 WHERE c.id = ?1",
                params![conversation_id],
                workspace_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn associated_worktree_for_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<Workspace>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT w.id, w.project_id, w.name, w.kind, w.root_path,
                        w.git_common_dir, w.base_ref, w.base_commit, w.branch,
                        w.local_environment_config_path, w.managed, w.pinned,
                        w.state, w.created_at, w.last_used_at
                 FROM conversations c
                 INNER JOIN workspaces w ON w.id = c.associated_worktree_id
                 WHERE c.id = ?1",
                params![conversation_id],
                workspace_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn insert_managed_workspace(
        &self,
        workspace: NewWorkspace<'_>,
    ) -> StorageResult<Workspace> {
        let id = new_uuid_v4();
        let now = now_string();
        let connection = self.connection.lock().expect("storage lock poisoned");
        project_by_id(&connection, workspace.project_id)?;
        connection.execute(
            "INSERT INTO workspaces
                (id, project_id, name, kind, root_path, git_common_dir,
                 base_ref, base_commit, branch, local_environment_config_path,
                 managed, pinned, state, created_at, last_used_at)
             VALUES (?1, ?2, ?3, 'worktree', ?4, ?5, ?6, ?7, ?8, ?9,
                     1, 0, 'available', ?10, ?10)",
            params![
                &id,
                workspace.project_id,
                normalize_project_name(workspace.name, workspace.root_path),
                workspace.root_path,
                workspace.git_common_dir,
                workspace.base_ref,
                workspace.base_commit,
                workspace.branch,
                workspace.local_environment_config_path,
                now,
            ],
        )?;
        workspace_by_id(&connection, &id).map_err(Into::into)
    }

    pub fn set_workspace_pinned(&self, id: &str, pinned: bool) -> StorageResult<Workspace> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE workspaces
             SET pinned = ?1, last_used_at = ?2
             WHERE id = ?3 AND kind = 'worktree'",
            params![if pinned { 1 } else { 0 }, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        workspace_by_id(&connection, id).map_err(Into::into)
    }

    pub fn set_workspace_branch(&self, id: &str, branch: &str) -> StorageResult<Workspace> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE workspaces
             SET name = ?1, branch = ?1, last_used_at = ?2
             WHERE id = ?3 AND kind = 'worktree'",
            params![branch, now_string(), id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        workspace_by_id(&connection, id).map_err(Into::into)
    }

    pub fn bind_conversation_workspace(
        &self,
        conversation_id: &str,
        workspace_id: &str,
    ) -> StorageResult<Workspace> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let workspace = workspace_by_id(&connection, workspace_id)?;
        let conversation_project: Option<i64> = connection.query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        if conversation_project != Some(workspace.project_id) {
            return Err(StorageError::Validation(
                "workspace does not belong to the conversation project".to_string(),
            ));
        }
        let now = now_string();
        let associated_worktree_id = (workspace.kind == "worktree").then_some(workspace_id);
        connection.execute(
            "UPDATE conversations
             SET workspace_id = ?1,
                 associated_worktree_id = COALESCE(?2, associated_worktree_id),
                 updated_at = ?3
             WHERE id = ?4",
            params![workspace_id, associated_worktree_id, &now, conversation_id],
        )?;
        connection.execute(
            "UPDATE workspaces SET last_used_at = ?1 WHERE id = ?2",
            params![now, workspace_id],
        )?;
        Ok(workspace)
    }

    pub fn handoff_conversation_workspace(
        &self,
        conversation_id: &str,
        workspace_id: &str,
        associated_worktree_id: &str,
    ) -> StorageResult<Workspace> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let workspace = workspace_by_id(&connection, workspace_id)?;
        let associated = workspace_by_id(&connection, associated_worktree_id)?;
        let conversation_project: Option<i64> = connection.query_row(
            "SELECT project_id FROM conversations WHERE id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        if conversation_project != Some(workspace.project_id)
            || associated.project_id != workspace.project_id
            || associated.kind != "worktree"
            || !associated.managed
        {
            return Err(StorageError::Validation(
                "handoff workspaces do not belong to this conversation".to_string(),
            ));
        }
        let now = now_string();
        connection.execute(
            "UPDATE conversations
             SET workspace_id = ?1, associated_worktree_id = ?2, updated_at = ?3
             WHERE id = ?4",
            params![workspace_id, associated_worktree_id, &now, conversation_id],
        )?;
        connection.execute(
            "UPDATE workspaces SET last_used_at = ?1 WHERE id IN (?2, ?3)",
            params![now, workspace_id, associated_worktree_id],
        )?;
        Ok(workspace)
    }

    pub fn workspace_conversation_count(&self, workspace_id: &str) -> StorageResult<u64> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM conversations
             WHERE workspace_id = ?1 OR associated_worktree_id = ?1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    pub fn delete_managed_workspace(&self, id: &str) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "DELETE FROM workspaces
             WHERE id = ?1 AND kind = 'worktree' AND managed = 1",
            params![id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        Ok(())
    }

    pub fn local_environment_approval(
        &self,
        project_id: i64,
        config_path: &str,
    ) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT content_sha256 FROM local_environment_approvals
                 WHERE project_id = ?1 AND config_path = ?2",
                params![project_id, config_path],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn set_local_environment_approval(
        &self,
        project_id: i64,
        config_path: &str,
        content_sha256: &str,
    ) -> StorageResult<()> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection.execute(
            "INSERT INTO local_environment_approvals
                (project_id, config_path, content_sha256, approved_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_id, config_path) DO UPDATE SET
                content_sha256 = excluded.content_sha256,
                approved_at = excluded.approved_at",
            params![project_id, config_path, content_sha256, now_string()],
        )?;
        Ok(())
    }

    pub fn set_conversation_approval_policy(
        &self,
        conversation_id: &str,
        policy: Option<&str>,
    ) -> StorageResult<ConversationSummary> {
        if let Some(policy) = policy {
            validate_approval_policy(policy)?;
        }
        let connection = self.connection.lock().expect("storage lock poisoned");
        let updated = connection.execute(
            "UPDATE conversations
             SET approval_policy_override = ?1, updated_at = ?2
             WHERE id = ?3",
            params![policy, now_string(), conversation_id],
        )?;
        if updated == 0 {
            return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
        }
        conversation_summary(&connection, conversation_id).map_err(Into::into)
    }

    pub fn conversation_approval_policy_override(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT approval_policy_override FROM conversations WHERE id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
    }

    pub fn remove_project(&self, id: i64) -> StorageResult<()> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        // Deregister only. The project directory and git checkout are never deleted here.
        transaction.execute(
            "UPDATE conversations SET agent_mode = 0 WHERE project_id = ?1",
            params![id],
        )?;
        // A Crux index state approves Crux indexing for its root. Delete the
        // states of the project root and of its workspace roots. Thus a later
        // project at one of these roots asks for approval again.
        transaction.execute(
            "DELETE FROM crux_index_state
             WHERE project_path IN (
                 SELECT root_path FROM projects WHERE id = ?1
                 UNION
                 SELECT root_path FROM workspaces WHERE project_id = ?1
             )",
            params![id],
        )?;
        transaction.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn project_for_conversation(
        &self,
        conversation_id: &str,
    ) -> StorageResult<Option<Project>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        connection
            .query_row(
                "SELECT p.id, p.name, COALESCE(w.root_path, p.root_path),
                        p.trust_level, p.approval_policy, p.created_at, p.last_opened_at
                 FROM conversations c
                 INNER JOIN projects p ON p.id = c.project_id
                 LEFT JOIN workspaces w ON w.id = c.workspace_id
                 WHERE c.id = ?1",
                params![conversation_id],
                project_from_row,
            )
            .optional()
            .map_err(Into::into)
    }
}

fn conversation_summary(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<ConversationSummary> {
    connection.query_row(
        "SELECT
            c.id,
            c.title,
            c.model,
            c.created_at,
            c.updated_at,
            c.archived_at,
            c.memories_mode,
            c.fast_mode,
         c.goal,
         c.agent_mode,
         c.workflow_mode,
         c.credential_profile_id,
         c.project_id,
            p.name AS project_name,
            COALESCE(w.root_path, p.root_path) AS project_root_path,
            p.trust_level AS project_trust_level,
                c.approval_policy_override,
                c.agent_backend,
                c.agent_session_model,
                COUNT(m.id) AS message_count,
                c.mode,
                c.title_source,
                c.pinned,
                c.draft,
                c.draft_only,
                c.workspace_id
         FROM conversations c
         LEFT JOIN projects p ON p.id = c.project_id
         LEFT JOIN workspaces w ON w.id = c.workspace_id
         LEFT JOIN messages m ON m.conversation_id = c.id
         WHERE c.id = ?1
        GROUP BY c.id",
        params![conversation_id],
        |row| {
            let fast_mode: Option<i64> = row.get(7)?;
            let agent_mode: i64 = row.get(9)?;
            let message_count: i64 = row.get(19)?;
            Ok(ConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                title_source: row.get(21)?,
                mode: row.get(20)?,
                model: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                pinned: row.get::<_, i64>(22)? != 0,
                draft: row.get(23)?,
                draft_only: row.get::<_, i64>(24)? != 0,
                archived_at: row.get(5)?,
                memories_mode: row.get(6)?,
                fast_mode: fast_mode.unwrap_or_default() != 0,
                goal: row.get(8)?,
                agent_mode: agent_mode != 0,
                workflow_mode: row.get(10)?,
                credential_profile_id: row.get(11)?,
                project_id: row.get(12)?,
                project_name: row.get(13)?,
                project_root_path: row.get(14)?,
                project_trust_level: row.get(15)?,
                approval_policy_override: row.get(16)?,
                agent_backend: row.get(17)?,
                agent_session_model: row.get(18)?,
                message_count: message_count.max(0) as u32,
                workspace_id: row.get(25)?,
            })
        },
    )
}

fn message_ids_from_anchor(
    connection: &Connection,
    conversation_id: &str,
    anchor_created_at: &str,
    anchor_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT id
         FROM messages
         WHERE conversation_id = ?1
           AND (created_at, id) >= (?2, ?3)
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = statement.query_map(
        params![conversation_id, anchor_created_at, anchor_id],
        |row| row.get(0),
    )?;
    rows.collect::<Result<Vec<_>, _>>()
}

fn message_attachments_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> StorageResult<HashMap<String, Vec<MessageAttachment>>> {
    let mut statement = connection.prepare(
        "SELECT a.id, a.message_id, a.name, a.media_type, a.size_bytes, a.data
         FROM message_attachments a
         INNER JOIN messages m ON m.id = a.message_id
         WHERE m.conversation_id = ?1
         ORDER BY m.created_at ASC, m.id ASC, a.position ASC, a.id ASC",
    )?;
    let rows = statement.query_map(params![conversation_id], |row| {
        let size_bytes = row.get::<_, i64>(4)?;
        let data = row.get::<_, Vec<u8>>(5)?;
        Ok((
            row.get::<_, String>(1)?,
            MessageAttachment {
                id: row.get(0)?,
                name: row.get(2)?,
                media_type: row.get(3)?,
                size_bytes: size_bytes.max(0) as u64,
                data: STANDARD.encode(data),
            },
        ))
    })?;
    let mut attachments = HashMap::<String, Vec<MessageAttachment>>::new();
    for row in rows {
        let (message_id, attachment) = row?;
        attachments.entry(message_id).or_default().push(attachment);
    }
    Ok(attachments)
}

fn delete_message_dependents(
    connection: &Connection,
    _conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<()> {
    for message_id in message_ids {
        connection.execute(
            "INSERT OR IGNORE INTO archived_savings_snapshots
                (message_id, message_role, tokens_saved, savings_usd, pricing_source,
                 created_at, crux_tokens_saved, occurred_local_date, usd_method,
                 price_version)
             SELECT s.message_id, m.role, s.tokens_saved, s.savings_usd,
                    s.pricing_source, m.created_at, s.crux_tokens_saved,
                    s.occurred_local_date, s.usd_method, s.price_version
             FROM savings_snapshots s
             INNER JOIN messages m ON m.id = s.message_id
             WHERE s.message_id = ?1",
            params![message_id],
        )?;
        connection.execute(
            "DELETE FROM savings_snapshots WHERE message_id = ?1",
            params![message_id],
        )?;
    }
    Ok(())
}

fn refresh_conversation_after_message_delete(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<()> {
    let updated_at: String = connection.query_row(
        "SELECT COALESCE(
            (SELECT MAX(created_at) FROM messages WHERE conversation_id = ?1),
            c.created_at
         )
         FROM conversations c
         WHERE c.id = ?1",
        params![conversation_id],
        |row| row.get(0),
    )?;
    connection.execute(
        "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
        params![updated_at, conversation_id],
    )?;
    Ok(())
}

fn project_by_id(connection: &Connection, id: i64) -> rusqlite::Result<Project> {
    connection.query_row(
        "SELECT id, name, root_path, trust_level, approval_policy, created_at, last_opened_at
         FROM projects
         WHERE id = ?1",
        params![id],
        project_from_row,
    )
}

fn project_by_root_path(connection: &Connection, root_path: &str) -> rusqlite::Result<Project> {
    connection.query_row(
        "SELECT id, name, root_path, trust_level, approval_policy, created_at, last_opened_at
         FROM projects
         WHERE root_path = ?1",
        params![root_path],
        project_from_row,
    )
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        root_path: row.get(2)?,
        trust_level: row.get(3)?,
        approval_policy: row.get(4)?,
        created_at: row.get(5)?,
        last_opened_at: row.get(6)?,
    })
}

fn credential_profile_by_id(
    connection: &Connection,
    id: &str,
) -> rusqlite::Result<CredentialProfile> {
    connection.query_row(
        "SELECT id, provider, name, kind, priority, status,
                last_validated_at, metadata_json, created_at, updated_at
         FROM credential_profiles
         WHERE id = ?1",
        params![id],
        credential_profile_from_row,
    )
}

fn credential_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CredentialProfile> {
    Ok(CredentialProfile {
        id: row.get(0)?,
        provider: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        priority: row.get(4)?,
        status: row.get(5)?,
        last_validated_at: row.get(6)?,
        metadata_json: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn agent_memory_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentMemory> {
    Ok(AgentMemory {
        id: row.get(0)?,
        project_id: row.get(1)?,
        project_name: row.get(2)?,
        content: row.get(3)?,
        source_conversation_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn workspace_by_id(connection: &Connection, id: &str) -> rusqlite::Result<Workspace> {
    connection.query_row(
        "SELECT id, project_id, name, kind, root_path, git_common_dir,
                base_ref, base_commit, branch, local_environment_config_path,
                managed, pinned, state,
                created_at, last_used_at
         FROM workspaces
         WHERE id = ?1",
        params![id],
        workspace_from_row,
    )
}

fn workspace_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    let managed: i64 = row.get(10)?;
    let pinned: i64 = row.get(11)?;
    Ok(Workspace {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        root_path: row.get(4)?,
        git_common_dir: row.get(5)?,
        base_ref: row.get(6)?,
        base_commit: row.get(7)?,
        branch: row.get(8)?,
        local_environment_config_path: row.get(9)?,
        managed: managed != 0,
        pinned: pinned != 0,
        state: row.get(12)?,
        created_at: row.get(13)?,
        last_used_at: row.get(14)?,
    })
}

fn ensure_primary_workspace(connection: &Connection, project: &Project) -> StorageResult<String> {
    let id = format!("primary-{}", project.id);
    connection.execute(
        "INSERT OR IGNORE INTO workspaces
            (id, project_id, name, kind, root_path, managed, pinned, state,
             created_at, last_used_at)
         VALUES (?1, ?2, ?3, 'primary', ?4, 0, 1, 'available', ?5, ?6)",
        params![
            &id,
            project.id,
            &project.name,
            &project.root_path,
            &project.created_at,
            project
                .last_opened_at
                .as_deref()
                .unwrap_or(&project.created_at),
        ],
    )?;
    Ok(id)
}

fn resolve_workspace_for_new_conversation(
    connection: &Connection,
    project_id: Option<i64>,
    workspace_id: Option<&str>,
) -> StorageResult<Option<String>> {
    match (
        project_id,
        workspace_id.map(str::trim).filter(|id| !id.is_empty()),
    ) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(StorageError::Validation(
            "a workspace requires a selected project".to_string(),
        )),
        (Some(project_id), Some(workspace_id)) => {
            let workspace = workspace_by_id(connection, workspace_id)?;
            if workspace.project_id != project_id || workspace.state != "available" {
                return Err(StorageError::Validation(
                    "workspace is unavailable or belongs to another project".to_string(),
                ));
            }
            Ok(Some(workspace.id))
        }
        (Some(project_id), None) => {
            let project = project_by_id(connection, project_id)?;
            ensure_primary_workspace(connection, &project).map(Some)
        }
    }
}

fn require_conversation_project(
    connection: &Connection,
    conversation_id: &str,
) -> StorageResult<()> {
    let row = connection
        .query_row(
            "SELECT c.project_id
             FROM conversations c
             WHERE c.id = ?1",
            params![conversation_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?;
    let Some(project_id) = row else {
        return Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows));
    };
    if project_id.is_none() {
        return Err(StorageError::Validation(
            "Agent mode requires a project-bound conversation".to_string(),
        ));
    }
    Ok(())
}

fn validate_trust_level(trust_level: &str) -> StorageResult<()> {
    match trust_level {
        "trusted" | "untrusted" => Ok(()),
        _ => Err(StorageError::Validation(
            "trust level must be `trusted` or `untrusted`".to_string(),
        )),
    }
}

fn validate_approval_policy(policy: &str) -> StorageResult<()> {
    match policy {
        "read_only" | "ask" | "on_request" | "on_failure" | "never" | "skip_all" => Ok(()),
        _ => Err(StorageError::Validation(
            "approval policy must be `read_only`, `ask`, `on_request`, `on_failure`, `never`, or `skip_all`".to_string(),
        )),
    }
}

fn validate_memories_mode(mode: Option<&str>) -> StorageResult<()> {
    match mode {
        None | Some("off" | "on") => Ok(()),
        Some(_) => Err(StorageError::Validation(
            "memories mode must be `off`, `on`, or null".to_string(),
        )),
    }
}

fn validate_credential_profile_status(status: &str) -> StorageResult<()> {
    match status {
        "active" | "disabled" | "invalid" => Ok(()),
        _ => Err(StorageError::Validation(
            "credential profile status must be `active`, `disabled`, or `invalid`".to_string(),
        )),
    }
}

fn message_pricing_context(
    connection: &Connection,
    message_id: &str,
) -> StorageResult<(Option<String>, Option<String>)> {
    let context = connection
        .query_row(
            "SELECT c.agent_backend, m.model
             FROM messages m
             INNER JOIN conversations c ON c.id = m.conversation_id
             WHERE m.id = ?1",
            params![message_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(context.unwrap_or_default())
}

struct SavingsSum {
    tokens_saved: i64,
    savings_usd: f64,
    pricing_source: Option<String>,
    dollars_usd: f64,
    legacy_tokens_saved: i64,
}

fn savings_sum<P>(connection: &Connection, sql: &str, params: P) -> rusqlite::Result<SavingsSum>
where
    P: rusqlite::Params,
{
    connection.query_row(sql, params, |row| {
        Ok(SavingsSum {
            tokens_saved: row.get(0)?,
            savings_usd: row.get(1)?,
            pricing_source: row.get(2)?,
            dollars_usd: row.get(3)?,
            legacy_tokens_saved: row.get(4)?,
        })
    })
}

fn recent_savings<P>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<(Vec<i64>, Vec<f64>)>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement
        .query_map(params, |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.reverse();
    Ok(rows.into_iter().unzip())
}

#[tauri::command]
pub async fn storage_list_conversations(
    storage: tauri::State<'_, Storage>,
) -> Result<Vec<ConversationSummary>, String> {
    storage
        .list_conversations()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_list_credential_profiles(
    storage: tauri::State<'_, Storage>,
) -> Result<Vec<CredentialProfile>, String> {
    storage
        .list_credential_profiles()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_list_usage_windows(
    storage: tauri::State<'_, Storage>,
) -> Result<Vec<UsageWindow>, String> {
    storage
        .list_usage_windows()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_list_profile_cooldowns(
    storage: tauri::State<'_, Storage>,
) -> Result<Vec<ProfileCooldown>, String> {
    storage
        .list_profile_cooldowns()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_get_messages(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Vec<StoredMessage>, String> {
    storage
        .get_messages(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_get_conversation_task_plan(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Option<ConversationTaskPlan>, String> {
    storage
        .conversation_task_plan(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_list_task_routes(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Vec<TaskRoute>, String> {
    storage
        .list_task_routes_for_conversation(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_get_latest_conversation_compaction(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<Option<ConversationCompaction>, String> {
    storage
        .latest_conversation_compaction(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn fork_conversation(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    upto_sequence: i64,
) -> Result<ConversationSummary, String> {
    storage
        .fork_conversation(&conversation_id, upto_sequence)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_delete_conversation(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<(), String> {
    storage
        .delete_conversation(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_title(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    title: String,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_title(&conversation_id, &title)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_pinned(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    pinned: bool,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_pinned(&conversation_id, pinned)
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn storage_create_conversation_draft(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    draft: String,
    model: String,
    project_id: Option<i64>,
    workspace_id: Option<String>,
    mode: String,
    agent_mode: bool,
    workflow_mode: String,
    credential_profile_id: Option<String>,
    agent_backend: Option<String>,
    agent_session_model: Option<String>,
    approval_policy: Option<String>,
    fast_mode: bool,
) -> Result<ConversationSummary, String> {
    storage
        .ensure_conversation_in_workspace(
            Some(conversation_id.clone()),
            &draft,
            &model,
            project_id,
            workspace_id.as_deref(),
        )
        .map_err(|error| error.to_string())?;
    let configured = (|| -> StorageResult<ConversationSummary> {
        if mode != "chat" {
            storage.set_conversation_mode(&conversation_id, &mode)?;
        }
        if agent_mode {
            storage.set_conversation_agent_mode(&conversation_id, true)?;
        }
        if workflow_mode != "default" {
            storage.set_conversation_workflow_mode(&conversation_id, &workflow_mode)?;
        }
        if credential_profile_id.is_some() {
            storage.set_conversation_credential_profile(&conversation_id, credential_profile_id)?;
        }
        if agent_backend.is_some() {
            storage.set_conversation_agent_backend(
                &conversation_id,
                agent_backend.as_deref(),
                agent_session_model.as_deref(),
            )?;
        }
        if approval_policy.is_some() {
            storage
                .set_conversation_approval_policy(&conversation_id, approval_policy.as_deref())?;
        }
        if fast_mode {
            storage.set_conversation_fast_mode(&conversation_id, true)?;
        }
        storage
            .set_conversation_draft(&conversation_id, &draft, true)?
            .ok_or_else(|| {
                StorageError::Validation("conversation draft cannot be empty".to_string())
            })
    })();
    match configured {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let _ = storage.delete_conversation(&conversation_id);
            Err(error.to_string())
        }
    }
}

#[tauri::command]
pub async fn storage_set_conversation_draft(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    draft: String,
) -> Result<Option<ConversationSummary>, String> {
    storage
        .set_conversation_draft(&conversation_id, &draft, false)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_consume_conversation_draft(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
) -> Result<ConversationSummary, String> {
    storage
        .consume_conversation_draft(&conversation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_mode(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    mode: String,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_mode(&conversation_id, &mode)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_archived(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    archived: bool,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_archived(&conversation_id, archived)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_memories_mode(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    mode: Option<String>,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_memories_mode(&conversation_id, mode.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_fast_mode(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    enabled: bool,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_fast_mode(&conversation_id, enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_agent_backend(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    agent_backend: Option<String>,
    agent_session_model: Option<String>,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_agent_backend(
            &conversation_id,
            agent_backend.as_deref(),
            agent_session_model.as_deref(),
        )
        .map_err(|error| error.to_string())
}

/// Clears the pin when `credential_profile_id` is `None`.
/// `chat_send` only changes the pin when it receives a profile ID.
#[tauri::command]
pub async fn storage_set_conversation_credential_profile(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    credential_profile_id: Option<String>,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_credential_profile(&conversation_id, credential_profile_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_list_agent_memories(
    storage: tauri::State<'_, Storage>,
) -> Result<Vec<AgentMemory>, String> {
    storage
        .list_agent_memories()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_delete_agent_memory(
    storage: tauri::State<'_, Storage>,
    id: i64,
) -> Result<bool, String> {
    storage
        .delete_agent_memory(id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_clear_agent_memories(
    storage: tauri::State<'_, Storage>,
) -> Result<usize, String> {
    storage
        .clear_agent_memories()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_truncate_from(
    chat: tauri::State<'_, crate::chat::ChatManager>,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    message_id: String,
) -> Result<ConversationSummary, StorageTruncateError> {
    crate::chat::truncate_conversation_from(
        chat.inner(),
        storage.inner(),
        &conversation_id,
        &message_id,
    )
    .await
}

#[tauri::command]
pub async fn storage_set_conversation_goal(
    storage: tauri::State<'_, Storage>,
    conversation_id: Option<String>,
    goal: Option<String>,
    model: Option<String>,
    project_id: Option<i64>,
    workspace_id: Option<String>,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_goal_in_workspace(
            conversation_id,
            goal.as_deref(),
            model.as_deref(),
            project_id,
            workspace_id.as_deref(),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_set_conversation_approval_policy(
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    policy: Option<String>,
) -> Result<ConversationSummary, String> {
    storage
        .set_conversation_approval_policy(&conversation_id, policy.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_total_savings(
    storage: tauri::State<'_, Storage>,
) -> Result<SavingsTotals, String> {
    storage.total_savings().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_daily_token_activity(
    storage: tauri::State<'_, Storage>,
    since_iso: String,
) -> Result<Vec<DailyTokenActivity>, String> {
    storage
        .daily_token_activity(&since_iso)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_savings_today(
    storage: tauri::State<'_, Storage>,
) -> Result<SavingsToday, String> {
    storage.savings_today().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn storage_savings_week(
    storage: tauri::State<'_, Storage>,
) -> Result<SavingsToday, String> {
    let since_iso = trailing_week_cutoff(Utc::now());
    storage
        .savings_since(&since_iso)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn search_messages(
    storage: tauri::State<'_, Storage>,
    query: String,
) -> Result<Vec<MessageSearchResult>, String> {
    storage
        .search_messages(&query)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn export_conversation_to_file(
    app: tauri::AppHandle,
    storage: tauri::State<'_, Storage>,
    conversation_id: String,
    format: ConversationExportFormat,
) -> Result<bool, String> {
    let summary = storage
        .conversation_summary(&conversation_id)
        .map_err(|error| error.to_string())?;
    let contents = storage
        .export_conversation(&conversation_id, format.clone())
        .map_err(|error| error.to_string())?;
    let file_name = export_file_name(&summary.title, &format);
    let (filter_name, extension) = match format {
        ConversationExportFormat::Markdown => ("Markdown", "md"),
        ConversationExportFormat::Json => ("JSON", "json"),
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export conversation")
        .set_file_name(file_name)
        .add_filter(filter_name, &[extension])
        .save_file(move |path| {
            let _ = sender.send(path);
        });

    let Some(file_path) = receiver
        .await
        .map_err(|_| "export file dialog closed unexpectedly".to_string())?
    else {
        return Ok(false);
    };
    let path = file_path
        .into_path()
        .map_err(|error| format!("selected export destination was not a local path: {error}"))?;
    let path_label = path.display().to_string();
    tokio::task::spawn_blocking(move || fs::write(&path, contents))
        .await
        .map_err(|error| format!("failed to join export writer: {error}"))?
        .map_err(|error| format!("failed to write export file `{path_label}`: {error}"))?;
    Ok(true)
}

fn export_file_name(title: &str, format: &ConversationExportFormat) -> String {
    let extension = match format {
        ConversationExportFormat::Markdown => "md",
        ConversationExportFormat::Json => "json",
    };
    let sanitized: String = title
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || r#"<>:"/\|?*"#.contains(character) {
                ' '
            } else {
                character
            }
        })
        .collect();
    let base: String = sanitized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(80)
        .collect();
    format!(
        "{}.{}",
        if base.is_empty() {
            "conversation"
        } else {
            &base
        },
        extension
    )
}

pub(crate) fn run_migrations(connection: &Connection) -> StorageResult<()> {
    // The legacy snapshot repair needs foreign_keys OFF, and SQLite ignores
    // that pragma inside a transaction. Toggle it before the transaction.
    let legacy_repair_expected = turn_snapshots_has_legacy_primary_key(connection)?;
    let restore_foreign_keys = if legacy_repair_expected {
        let enabled: bool =
            connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        if enabled {
            connection.pragma_update(None, "foreign_keys", "OFF")?;
        }
        enabled
    } else {
        false
    };

    // BEGIN IMMEDIATE serializes concurrent migrators (overlapping tauri-dev
    // restarts share this DB); busy_timeout makes the loser wait, and
    // INSERT OR IGNORE tolerates a racer that already recorded the version.
    // The schema bootstrap and the legacy repair run inside this transaction
    // so overlapping app starts cannot race the repair step.
    connection.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> StorageResult<()> {
        connection.execute_batch(MIGRATIONS_SQL)?;
        repair_legacy_turn_snapshots_primary_key(connection)?;

        let already_applied: Option<String> = connection
            .query_row(
                "SELECT version FROM migrations WHERE version = ?1",
                params![MIGRATION_INITIAL],
                |row| row.get(0),
            )
            .optional()?;

        if already_applied.is_none() {
            connection.execute_batch(INITIAL_SCHEMA_SQL)?;
            connection.execute(
                "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_INITIAL, now_string()],
            )?;
        }

        apply_conversation_goal_migration(connection)?;
        apply_projects_migration(connection)?;
        apply_agent_mode_trust_migration(connection)?;
        apply_approval_policy_migration(connection)?;
        apply_turn_diffs_migration(connection)?;
        apply_turn_snapshot_postconditions_migration(connection)?;
        apply_message_mentions_migration(connection)?;
        apply_approval_policy_skip_all_migration(connection)?;
        apply_auth_session_migration(connection)?;
        apply_search_and_titles_migration(connection)?;
        apply_workspaces_migration(connection)?;
        apply_archived_savings_migration(connection)?;
        apply_message_attachments_migration(connection)?;
        apply_conversation_workflow_migration(connection)?;
        apply_subagent_threads_migration(connection)?;
        apply_workspace_local_environment_migration(connection)?;
        apply_conversation_worktree_handoff_migration(connection)?;
        apply_conversation_compactions_migration(connection)?;
        apply_approval_policy_on_failure_migration(connection)?;
        apply_conversation_archive_migration(connection)?;
        apply_agent_memories_migration(connection)?;
        apply_conversation_fast_mode_migration(connection)?;
        apply_local_diagnostics_migration(connection)?;
        apply_savings_report_queue_migration(connection)?;
        apply_crux_index_state_migration(connection)?;
        apply_crux_savings_migration(connection)?;
        apply_clear_project_approval_policies_migration(connection)?;
        apply_free_turns_migration(connection)?;
        apply_credential_profiles_migration(connection)?;
        apply_usage_windows_migration(connection)?;
        apply_agent_sessions_migration(connection)?;
        apply_conversation_agent_backend_migration(connection)?;
        apply_agent_message_usage_migration(connection)?;
        apply_agent_session_custom_title_migration(connection)?;
        apply_agent_session_conversation_migration(connection)?;
        apply_conversation_mode_agent_titles_migration(connection)?;
        apply_first_launch_at_migration(connection)?;
        apply_conversation_pinned_migration(connection)?;
        apply_conversation_draft_migration(connection)?;
        apply_ponytail_mode_migration(connection)?;
        apply_agent_session_crux_savings_migration(connection)?;
        apply_savings_usd_pricing_migration(connection)?;
        apply_agent_session_message_snapshot_migration(connection)?;
        apply_savings_local_date_migration(connection)?;
        apply_agent_session_savings_paths_migration(connection)?;
        apply_savings_report_usd_migration(connection)?;
        apply_session_checkpoints_migration(connection)?;
        apply_task_routes_migration(connection)?;
        apply_task_routes_worker_lane_migration(connection)?;
        apply_task_routes_effort_migration(connection)?;
        // Each migration checks its own version row, so the call order is free.
        workspace_tabs::apply_workspace_tabs_migration(connection)?;
        apply_task_hierarchies_migration(connection)?;
        apply_remote_devices_migration(connection)?;
        apply_local_environment_approvals_migration(connection)?;
        apply_task_route_receipts_migration(connection)?;
        apply_agent_session_headroom_route_migration(connection)?;
        savings_dollars::apply_savings_dollars_migration(connection)?;
        routing_savings::apply_routing_savings_migration(connection)?;
        session_token::apply_auth_session_token_account_migration(connection)?;
        Ok(())
    })();

    let outcome = match result {
        Ok(()) => connection
            .execute_batch("COMMIT")
            .map_err(StorageError::from),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    };

    if restore_foreign_keys {
        let restored = connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(StorageError::from);
        return outcome.and(restored);
    }
    outcome
}

fn apply_conversation_goal_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_GOAL],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !conversation_goal_column_exists(connection)? {
        connection.execute_batch(CONVERSATION_GOAL_SQL)?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_GOAL, now_string()],
    )?;
    Ok(())
}

fn conversation_goal_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "goal")
}

fn apply_projects_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_PROJECTS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            root_path TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            last_opened_at TEXT
        );",
    )?;

    if !project_id_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL;",
        )?;
    }

    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_projects_last_opened_at
            ON projects(COALESCE(last_opened_at, created_at) DESC, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_conversations_project_id
            ON conversations(project_id);",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_PROJECTS, now_string()],
    )?;
    Ok(())
}

fn apply_agent_mode_trust_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_MODE_TRUST],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !agent_mode_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN agent_mode INTEGER NOT NULL DEFAULT 0 CHECK (agent_mode IN (0, 1));",
        )?;
    }
    if !project_trust_level_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE projects
                ADD COLUMN trust_level TEXT CHECK (trust_level IN ('trusted', 'untrusted'));",
        )?;
    }

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_MODE_TRUST, now_string()],
    )?;
    Ok(())
}

fn apply_approval_policy_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_APPROVAL_POLICY],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !project_approval_policy_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE projects
                ADD COLUMN approval_policy TEXT
                CHECK (approval_policy IN ('read_only', 'ask', 'on_request', 'never'));",
        )?;
    }
    if !conversation_approval_policy_override_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN approval_policy_override TEXT
                CHECK (approval_policy_override IN ('read_only', 'ask', 'on_request', 'never'));",
        )?;
    }

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_APPROVAL_POLICY, now_string()],
    )?;
    Ok(())
}

fn apply_approval_policy_skip_all_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_APPROVAL_POLICY_SKIP_ALL],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if project_approval_policy_column_exists(connection)? {
        rebuild_approval_policy_column(connection, "projects", "approval_policy")?;
    }
    if conversation_approval_policy_override_column_exists(connection)? {
        rebuild_approval_policy_column(connection, "conversations", "approval_policy_override")?;
    }

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_APPROVAL_POLICY_SKIP_ALL, now_string()],
    )?;
    Ok(())
}

fn rebuild_approval_policy_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> StorageResult<()> {
    let new_column = format!("{column}_new");
    connection.execute_batch(&format!(
        "ALTER TABLE {table}
            ADD COLUMN {new_column} TEXT
            CHECK ({new_column} IN ('read_only', 'ask', 'on_request', 'on_failure', 'never', 'skip_all'));
         UPDATE {table} SET {new_column} = {column};
         ALTER TABLE {table} DROP COLUMN {column};
         ALTER TABLE {table} RENAME COLUMN {new_column} TO {column};"
    ))?;
    Ok(())
}

fn apply_approval_policy_on_failure_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_APPROVAL_POLICY_ON_FAILURE],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if project_approval_policy_column_exists(connection)? {
        rebuild_approval_policy_column(connection, "projects", "approval_policy")?;
    }
    if conversation_approval_policy_override_column_exists(connection)? {
        rebuild_approval_policy_column(connection, "conversations", "approval_policy_override")?;
    }

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_APPROVAL_POLICY_ON_FAILURE, now_string()],
    )?;
    Ok(())
}

fn apply_clear_project_approval_policies_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CLEAR_PROJECT_APPROVAL_POLICIES],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if project_approval_policy_column_exists(connection)? {
        connection.execute(
            "UPDATE projects
             SET approval_policy = NULL
             WHERE approval_policy IN ('on_request', 'never', 'skip_all')",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CLEAR_PROJECT_APPROVAL_POLICIES, now_string()],
    )?;
    Ok(())
}

fn apply_free_turns_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_FREE_TURNS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "local_diagnostics", "free_turns_used")? {
        connection.execute(
            "ALTER TABLE local_diagnostics
             ADD COLUMN free_turns_used INTEGER NOT NULL DEFAULT 0
             CHECK (free_turns_used >= 0)",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_FREE_TURNS, now_string()],
    )?;
    Ok(())
}

fn apply_first_launch_at_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_FIRST_LAUNCH_AT],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "local_diagnostics", "first_launch_at")? {
        connection.execute(
            "ALTER TABLE local_diagnostics ADD COLUMN first_launch_at TEXT",
            [],
        )?;
    }
    connection.execute(
        "UPDATE local_diagnostics
         SET first_launch_at = first_activated_at
         WHERE first_launch_at IS NULL",
        [],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_FIRST_LAUNCH_AT, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_pinned_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_PINNED],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "conversations", "pinned")? {
        connection.execute(
            "ALTER TABLE conversations
             ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0
             CHECK (pinned IN (0, 1))",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_PINNED, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_draft_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_DRAFT],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "conversations", "draft")? {
        connection.execute(
            "ALTER TABLE conversations ADD COLUMN draft TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !column_exists(connection, "conversations", "draft_only")? {
        connection.execute(
            "ALTER TABLE conversations
             ADD COLUMN draft_only INTEGER NOT NULL DEFAULT 0
             CHECK (draft_only IN (0, 1))",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_DRAFT, now_string()],
    )?;
    Ok(())
}

fn apply_agent_sessions_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSIONS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(AGENT_SESSIONS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSIONS, now_string()],
    )?;
    Ok(())
}

fn apply_agent_session_crux_savings_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSION_CRUX_SAVINGS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "agent_session_turns", "crux_tokens_saved")? {
        connection.execute(
            "ALTER TABLE agent_session_turns
             ADD COLUMN crux_tokens_saved INTEGER NOT NULL DEFAULT 0
             CHECK (crux_tokens_saved >= 0)",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_CRUX_SAVINGS, now_string()],
    )?;
    Ok(())
}

fn apply_savings_usd_pricing_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SAVINGS_USD_PRICING],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "agent_session_turns", "savings_usd")? {
        connection.execute(
            "ALTER TABLE agent_session_turns ADD COLUMN savings_usd REAL NULL",
            [],
        )?;
    }
    if !column_exists(connection, "agent_session_turns", "pricing_source")? {
        connection.execute(
            "ALTER TABLE agent_session_turns
             ADD COLUMN pricing_source TEXT NULL CHECK (
               pricing_source IN ('headroom', 'model_table', 'fallback')
                 OR pricing_source IS NULL
             )",
            [],
        )?;
    }
    for table in ["savings_snapshots", "archived_savings_snapshots"] {
        if !column_exists(connection, table, "pricing_source")? {
            connection.execute(
                &format!(
                    "ALTER TABLE {table}
                     ADD COLUMN pricing_source TEXT NULL CHECK (
                       pricing_source IN ('headroom', 'model_table', 'fallback')
                         OR pricing_source IS NULL
                     )"
                ),
                [],
            )?;
        }
    }

    backfill_agent_turn_savings_usd(connection)?;
    backfill_snapshot_savings_usd(connection)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SAVINGS_USD_PRICING, now_string()],
    )?;
    Ok(())
}

fn apply_agent_session_message_snapshot_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSION_MESSAGE_SNAPSHOT],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "agent_session_turns", "has_message_snapshot")? {
        connection.execute(
            "ALTER TABLE agent_session_turns
             ADD COLUMN has_message_snapshot INTEGER NULL
             CHECK (has_message_snapshot IN (0, 1))",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_MESSAGE_SNAPSHOT, now_string()],
    )?;
    Ok(())
}

fn apply_savings_local_date_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SAVINGS_LOCAL_DATE],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    for table in [
        "agent_session_turns",
        "savings_snapshots",
        "archived_savings_snapshots",
    ] {
        if !column_exists(connection, table, "occurred_local_date")? {
            connection.execute(
                &format!(
                    "ALTER TABLE {table}
                     ADD COLUMN occurred_local_date TEXT NULL CHECK (
                       occurred_local_date IS NULL
                         OR occurred_local_date GLOB
                           '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
                     )"
                ),
                [],
            )?;
        }
    }
    connection.execute(
        "UPDATE agent_session_turns
         SET occurred_local_date = date(created_at, 'localtime')
         WHERE occurred_local_date IS NULL",
        [],
    )?;
    connection.execute(
        "UPDATE savings_snapshots
         SET occurred_local_date = (
           SELECT date(m.created_at, 'localtime')
           FROM messages m
           WHERE m.id = savings_snapshots.message_id
         )
         WHERE occurred_local_date IS NULL",
        [],
    )?;
    connection.execute(
        "UPDATE archived_savings_snapshots
         SET occurred_local_date = date(created_at, 'localtime')
         WHERE occurred_local_date IS NULL",
        [],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SAVINGS_LOCAL_DATE, now_string()],
    )?;
    Ok(())
}

fn apply_agent_session_savings_paths_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSION_SAVINGS_PATHS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "agent_sessions", "rtk_db_path")? {
        connection.execute(
            "ALTER TABLE agent_sessions ADD COLUMN rtk_db_path TEXT NULL",
            [],
        )?;
    }
    if !column_exists(connection, "agent_sessions", "crux_ledger_path")? {
        connection.execute(
            "ALTER TABLE agent_sessions ADD COLUMN crux_ledger_path TEXT NULL",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_SAVINGS_PATHS, now_string()],
    )?;
    Ok(())
}

fn apply_session_checkpoints_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SESSION_CHECKPOINTS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(SESSION_CHECKPOINTS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SESSION_CHECKPOINTS, now_string()],
    )?;
    Ok(())
}

fn apply_task_routes_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TASK_ROUTES],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(TASK_ROUTES_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TASK_ROUTES, now_string()],
    )?;
    Ok(())
}

fn apply_task_routes_worker_lane_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TASK_ROUTES_WORKER_LANE],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "task_routes", "worker_lane")? {
        connection.execute(
            "ALTER TABLE task_routes
             ADD COLUMN worker_lane TEXT NOT NULL DEFAULT 'auto'",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TASK_ROUTES_WORKER_LANE, now_string()],
    )?;
    Ok(())
}

fn apply_task_routes_effort_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TASK_ROUTES_EFFORT],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "task_routes", "effort")? {
        connection.execute("ALTER TABLE task_routes ADD COLUMN effort TEXT NULL", [])?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TASK_ROUTES_EFFORT, now_string()],
    )?;
    Ok(())
}

fn apply_task_hierarchies_migration(connection: &Connection) -> StorageResult<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_hierarchies (
            session_id TEXT PRIMARY KEY,
            graph_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TASK_HIERARCHIES, now_string()],
    )?;
    Ok(())
}

// The device registry of the remote gateway (docs/remote-protocol.md, section 9).
// Times are epoch milliseconds, as in the protocol. `crate::remote::store` owns the queries.
// `host_id` is the host identity that paired the device. Debug and release builds use
// different identities, so each query filters by the host id.
fn apply_remote_devices_migration(connection: &Connection) -> StorageResult<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_devices (
            id TEXT PRIMARY KEY,
            host_id TEXT NOT NULL,
            name TEXT NOT NULL,
            platform TEXT NOT NULL,
            model TEXT NULL,
            noise_public BLOB NOT NULL,
            token_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_seen_at INTEGER NULL,
            revoked_at INTEGER NULL,
            UNIQUE (host_id, noise_public)
        );",
    )?;
    // Development builds made this table without `host_id` before a release.
    // Their rows cannot name a host, so they get an empty host id and no host sees them.
    if !column_exists(connection, "remote_devices", "host_id")? {
        connection.execute_batch(
            "ALTER TABLE remote_devices ADD COLUMN host_id TEXT NOT NULL DEFAULT '';",
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_REMOTE_DEVICES, now_string()],
    )?;
    Ok(())
}

/// Adds `agent_sessions.headroom_route`. Rows from 1.0.0 or older keep
/// NULL: the Halv Engine was always on then, so the lane tells the route.
fn apply_agent_session_headroom_route_migration(connection: &Connection) -> StorageResult<()> {
    if !column_exists(connection, "agent_sessions", "headroom_route")? {
        connection.execute(
            "ALTER TABLE agent_sessions ADD COLUMN headroom_route TEXT NULL CHECK (
               headroom_route IN ('direct', 'engine', 'workers', 'isolated')
               OR headroom_route IS NULL
             )",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_HEADROOM_ROUTE, now_string()],
    )?;
    Ok(())
}

fn apply_task_route_receipts_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TASK_ROUTE_RECEIPTS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    // ADD COLUMN only: a savings MCP server of an older bundle still reads
    // this table (ADR 0012).
    for (name, definition) in TASK_ROUTE_RECEIPT_ROUTE_COLUMNS {
        if !column_exists(connection, "task_routes", name)? {
            connection.execute(
                &format!("ALTER TABLE task_routes ADD COLUMN {name} {definition}"),
                [],
            )?;
        }
    }
    connection.execute_batch(TASK_ROUTE_RECEIPTS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TASK_ROUTE_RECEIPTS, now_string()],
    )?;
    Ok(())
}

fn apply_local_environment_approvals_migration(connection: &Connection) -> StorageResult<()> {
    connection.execute_batch(LOCAL_ENVIRONMENT_APPROVALS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_LOCAL_ENVIRONMENT_APPROVALS, now_string()],
    )?;
    Ok(())
}

/// True when the database has the workflow columns of migration 0056. A
/// savings MCP server of a newer bundle can meet a database that the running
/// app did not migrate yet.
pub(crate) fn task_route_workflow_columns_exist(connection: &Connection) -> rusqlite::Result<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('task_routes') WHERE name = 'batch_id'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

const SESSION_ENDED_REASON: &str =
    "Session ended without a task result. Completion was not confirmed.";

// The caller holds a transaction so route records and their graph change together.
fn close_session_routes(connection: &Connection, session_id: &str) -> StorageResult<()> {
    let graph_json: Option<String> = connection
        .query_row(
            "SELECT graph_json FROM task_hierarchies WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()?;
    let updated_at = now_string();
    if let Some(graph_json) = graph_json {
        let mut graph: crate::task_hierarchy::Graph = serde_json::from_str(&graph_json)
            .map_err(|error| StorageError::Validation(error.to_string()))?;
        graph.validate().map_err(StorageError::Validation)?;
        if graph.close_session() {
            graph.validate().map_err(StorageError::Validation)?;
            let json = serde_json::to_string(&graph)
                .map_err(|error| StorageError::Validation(error.to_string()))?;
            connection.execute(
                "UPDATE task_hierarchies SET graph_json = ?2, updated_at = ?3 WHERE session_id = ?1",
                params![session_id, json, updated_at],
            )?;
        }
    }
    for route in list_task_routes_rows(connection, "session_id = ?1", session_id)? {
        if route.status != "routed" {
            continue;
        }
        let mut reasons = route.reasons;
        reasons.push(SESSION_ENDED_REASON.to_string());
        let reasons = serde_json::to_string(&reasons)
            .map_err(|error| StorageError::Validation(error.to_string()))?;
        connection.execute(
            "UPDATE task_routes
             SET status = 'failed', closed_unreported = 1, reasons_json = ?2, updated_at = ?3
             WHERE id = ?1 AND status = 'routed'",
            params![route.id, reasons, updated_at],
        )?;
    }
    Ok(())
}

const TASK_ROUTE_COLUMNS: &str = "id, session_id, conversation_id, title, brief_chars,
     complexity, complexity_confidence, needs_reasoning, risky,
     tier, lane, model, effort, reasons_json, worker_lane, status, escalated_from_id,
     provider, provider_model, latency_ms, created_at, updated_at,
     batch_id, workflow_name, route_key, role, instances,
     default_model, default_effort, default_source, closed_unreported";

/// Writes one routed task. The MCP process shares this SQL through its own
/// connection.
pub(crate) fn insert_task_route_row(
    connection: &Connection,
    route: &TaskRoute,
) -> rusqlite::Result<()> {
    let reasons_json = serde_json::to_string(&route.reasons).unwrap_or_else(|_| "[]".to_string());
    let workflow = route.workflow.as_ref();
    connection.execute(
        "INSERT INTO task_routes (
            id, session_id, conversation_id, title, brief_chars,
            complexity, complexity_confidence, needs_reasoning, risky,
            tier, lane, model, effort, reasons_json, worker_lane, status,
            escalated_from_id, provider, provider_model, latency_ms,
            created_at, updated_at, batch_id, workflow_name, route_key, role,
            instances, default_model, default_effort, default_source,
            closed_unreported
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
            ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31
         )",
        params![
            route.id,
            route.session_id,
            route.conversation_id,
            route.title,
            route.brief_chars,
            route.complexity,
            route.complexity_confidence,
            route.needs_reasoning,
            route.risky,
            route.tier,
            route.lane,
            route.model,
            route.effort,
            reasons_json,
            route.worker_lane,
            route.status,
            route.escalated_from_id,
            route.provider,
            route.provider_model,
            route.latency_ms,
            route.created_at,
            route.updated_at,
            workflow.map(|workflow| workflow.batch_id.as_str()),
            workflow.and_then(|workflow| workflow.name.as_deref()),
            workflow.map(|workflow| workflow.key.as_str()),
            workflow.map(|workflow| workflow.role.as_str()),
            workflow.map_or(1, |workflow| workflow.instances),
            route.default_model,
            route.default_effort,
            route.default_source,
            route.closed_unreported,
        ],
    )?;
    Ok(())
}

pub(crate) fn update_task_route_status_row(
    connection: &Connection,
    id: &str,
    status: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE task_routes SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, now_string(), id],
    )
}

pub(crate) fn task_route_row(
    connection: &Connection,
    id: &str,
) -> rusqlite::Result<Option<TaskRoute>> {
    let mut routes = list_task_routes_rows(connection, "id = ?1", id)?;
    Ok(routes.pop())
}

/// Lists routes. `filter` is a condition on `task_routes` with one parameter.
pub(crate) fn list_task_routes_rows(
    connection: &Connection,
    filter: &str,
    value: &str,
) -> rusqlite::Result<Vec<TaskRoute>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {TASK_ROUTE_COLUMNS} FROM task_routes
         WHERE {filter}
         ORDER BY created_at ASC, id ASC"
    ))?;
    let mut routes = statement
        .query_map(params![value], task_route_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if routes.iter().any(|route| route.workflow.is_some()) {
        let mut receipts = task_route_receipt_rows(connection, filter, value)?;
        for route in &mut routes {
            route.receipt = receipts.remove(&route.id);
        }
    }
    Ok(routes)
}

const TASK_ROUTE_RECEIPT_COLUMNS: &str = "r.route_id, r.session_id, r.outcome, r.agents,
     r.failed_agents, r.input_tokens, r.output_tokens, r.cache_read_tokens,
     r.cache_creation_tokens, r.duration_ms, r.usage_source, r.reported_at,
     (SELECT group_concat(observed_model, char(10)) FROM (
         SELECT DISTINCT a.observed_model FROM task_route_agents a
         WHERE a.route_id = r.route_id ORDER BY a.observed_model
     ))";

fn task_route_receipt_rows(
    connection: &Connection,
    filter: &str,
    value: &str,
) -> rusqlite::Result<HashMap<String, TaskRouteReceipt>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {TASK_ROUTE_RECEIPT_COLUMNS} FROM task_route_receipts r
         WHERE r.route_id IN (SELECT id FROM task_routes WHERE {filter})"
    ))?;
    let rows = statement.query_map(params![value], |row| {
        let models: Option<String> = row.get(12)?;
        Ok(TaskRouteReceipt {
            route_id: row.get(0)?,
            session_id: row.get(1)?,
            outcome: row.get(2)?,
            agents: row.get(3)?,
            failed_agents: row.get(4)?,
            input_tokens: row.get(5)?,
            output_tokens: row.get(6)?,
            cache_read_tokens: row.get(7)?,
            cache_creation_tokens: row.get(8)?,
            duration_ms: row.get(9)?,
            usage_source: row.get(10)?,
            reported_at: row.get(11)?,
            models: models
                .map(|models| models.lines().map(str::to_owned).collect())
                .unwrap_or_default(),
        })
    })?;
    rows.map(|receipt| receipt.map(|receipt| (receipt.route_id.clone(), receipt)))
        .collect()
}

/// Writes or replaces the receipt of one route.
pub(crate) fn upsert_task_route_receipt_row(
    connection: &Connection,
    receipt: &TaskRouteReceipt,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO task_route_receipts (
            route_id, session_id, outcome, agents, failed_agents, input_tokens,
            output_tokens, cache_read_tokens, cache_creation_tokens, duration_ms,
            usage_source, reported_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(route_id) DO UPDATE SET
            outcome = excluded.outcome,
            agents = excluded.agents,
            failed_agents = excluded.failed_agents,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            cache_creation_tokens = excluded.cache_creation_tokens,
            duration_ms = excluded.duration_ms,
            usage_source = excluded.usage_source,
            reported_at = excluded.reported_at",
        params![
            receipt.route_id,
            receipt.session_id,
            receipt.outcome,
            receipt.agents,
            receipt.failed_agents,
            receipt.input_tokens,
            receipt.output_tokens,
            receipt.cache_read_tokens,
            receipt.cache_creation_tokens,
            receipt.duration_ms,
            receipt.usage_source,
            receipt.reported_at,
        ],
    )?;
    Ok(())
}

/// Replaces the measured agents of one route.
pub(crate) fn replace_task_route_agent_rows(
    connection: &Connection,
    route_id: &str,
    agents: &[TaskRouteAgent],
) -> rusqlite::Result<()> {
    connection.execute(
        "DELETE FROM task_route_agents WHERE route_id = ?1",
        params![route_id],
    )?;
    for agent in agents {
        connection.execute(
            "INSERT INTO task_route_agents (
                route_id, agent_id, observed_model, workflow_run_id, label,
                observed_effort, input_tokens, output_tokens, cache_read_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, started_at,
                ended_at, observed_at, actual_usd, price_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                route_id,
                agent.agent_id,
                agent.observed_model,
                agent.workflow_run_id,
                agent.label,
                agent.observed_effort,
                agent.input_tokens,
                agent.output_tokens,
                agent.cache_read_tokens,
                agent.cache_write_5m_tokens,
                agent.cache_write_1h_tokens,
                agent.started_at,
                agent.ended_at,
                agent.observed_at,
                agent.actual_usd,
                agent.price_version,
            ],
        )?;
    }
    Ok(())
}

/// Lists the measured agents of one route.
pub(crate) fn list_task_route_agent_rows(
    connection: &Connection,
    route_id: &str,
) -> rusqlite::Result<Vec<TaskRouteAgent>> {
    let mut statement = connection.prepare(
        "SELECT route_id, agent_id, observed_model, workflow_run_id, label,
                observed_effort, input_tokens, output_tokens, cache_read_tokens,
                cache_write_5m_tokens, cache_write_1h_tokens, started_at,
                ended_at, observed_at, actual_usd, price_version
         FROM task_route_agents WHERE route_id = ?1
         ORDER BY agent_id, observed_model",
    )?;
    let rows = statement.query_map(params![route_id], |row| {
        Ok(TaskRouteAgent {
            route_id: row.get(0)?,
            agent_id: row.get(1)?,
            observed_model: row.get(2)?,
            workflow_run_id: row.get(3)?,
            label: row.get(4)?,
            observed_effort: row.get(5)?,
            input_tokens: row.get(6)?,
            output_tokens: row.get(7)?,
            cache_read_tokens: row.get(8)?,
            cache_write_5m_tokens: row.get(9)?,
            cache_write_1h_tokens: row.get(10)?,
            started_at: row.get(11)?,
            ended_at: row.get(12)?,
            observed_at: row.get(13)?,
            actual_usd: row.get(14)?,
            price_version: row.get(15)?,
        })
    })?;
    rows.collect()
}

fn task_route_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRoute> {
    let reasons_json: String = row.get(13)?;
    let reasons = serde_json::from_str::<Vec<String>>(&reasons_json).unwrap_or_default();
    let batch_id: Option<String> = row.get(22)?;
    let workflow = match batch_id {
        Some(batch_id) => Some(TaskRouteWorkflow {
            batch_id,
            name: row.get(23)?,
            key: row.get::<_, Option<String>>(24)?.unwrap_or_default(),
            role: row.get::<_, Option<String>>(25)?.unwrap_or_default(),
            instances: row.get(26)?,
        }),
        None => None,
    };
    Ok(TaskRoute {
        id: row.get(0)?,
        session_id: row.get(1)?,
        conversation_id: row.get(2)?,
        title: row.get(3)?,
        brief_chars: row.get(4)?,
        complexity: row.get(5)?,
        complexity_confidence: row.get(6)?,
        needs_reasoning: row.get(7)?,
        risky: row.get(8)?,
        tier: row.get(9)?,
        lane: row.get(10)?,
        model: row.get(11)?,
        effort: row.get(12)?,
        reasons,
        worker_lane: row.get(14)?,
        status: row.get(15)?,
        escalated_from_id: row.get(16)?,
        provider: row.get(17)?,
        provider_model: row.get(18)?,
        latency_ms: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
        default_model: row.get(27)?,
        default_effort: row.get(28)?,
        default_source: row.get(29)?,
        closed_unreported: row.get(30)?,
        workflow,
        receipt: None,
    })
}

fn backfill_agent_turn_savings_usd(connection: &Connection) -> StorageResult<()> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT t.id, t.savings_tokens, a.agent_kind,
                    COALESCE(c.agent_session_model, c.model)
             FROM agent_session_turns t
             INNER JOIN agent_sessions a ON a.id = t.session_id
             LEFT JOIN conversations c ON c.id = a.conversation_id
             WHERE t.savings_tokens > 0
               AND (t.savings_usd IS NULL OR t.pricing_source IS NULL)",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (id, tokens, agent_kind, model) in rows {
        let price = pricing::legacy::input_price(Some(&agent_kind), model.as_deref());
        connection.execute(
            "UPDATE agent_session_turns
             SET savings_usd = COALESCE(savings_usd, ?1),
                 pricing_source = COALESCE(pricing_source, ?2)
             WHERE id = ?3",
            params![price.savings_usd(tokens), price.source.as_str(), id],
        )?;
    }
    Ok(())
}

fn backfill_snapshot_savings_usd(connection: &Connection) -> StorageResult<()> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT s.message_id, s.tokens_saved, s.savings_usd,
                    c.agent_backend, m.model
             FROM savings_snapshots s
             INNER JOIN messages m ON m.id = s.message_id
             INNER JOIN conversations c ON c.id = m.conversation_id
             WHERE s.tokens_saved > 0 AND s.pricing_source IS NULL",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (message_id, tokens, savings_usd, agent_kind, model) in rows {
        let (savings_usd, source) = savings_usd.map_or_else(
            || {
                let price = pricing::legacy::input_price(agent_kind.as_deref(), model.as_deref());
                (price.savings_usd(tokens), price.source)
            },
            |savings_usd| (savings_usd, PricingSource::Headroom),
        );
        connection.execute(
            "UPDATE savings_snapshots
             SET savings_usd = ?1, pricing_source = ?2
             WHERE message_id = ?3",
            params![savings_usd, source.as_str(), message_id],
        )?;
    }

    connection.execute(
        "UPDATE archived_savings_snapshots
         SET pricing_source = 'headroom'
         WHERE savings_usd IS NOT NULL AND pricing_source IS NULL",
        [],
    )?;
    let fallback = pricing::legacy::fallback_input_price();
    connection.execute(
        "UPDATE archived_savings_snapshots
         SET savings_usd = MAX(COALESCE(tokens_saved, 0), 0) * ?1,
             pricing_source = 'fallback'
         WHERE savings_usd IS NULL AND tokens_saved > 0",
        params![fallback.savings_usd(1)],
    )?;
    Ok(())
}

fn apply_ponytail_mode_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_PONYTAIL_MODE],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    connection.execute_batch(PONYTAIL_MODE_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_PONYTAIL_MODE, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_agent_backend_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_AGENT_BACKEND],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "conversations", "agent_backend")? {
        connection.execute_batch("ALTER TABLE conversations ADD COLUMN agent_backend TEXT;")?;
    }
    if !column_exists(connection, "conversations", "agent_session_model")? {
        connection
            .execute_batch("ALTER TABLE conversations ADD COLUMN agent_session_model TEXT;")?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_AGENT_BACKEND, now_string()],
    )?;
    Ok(())
}

fn apply_agent_message_usage_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_MESSAGE_USAGE],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    connection.execute_batch(AGENT_MESSAGE_USAGE_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_MESSAGE_USAGE, now_string()],
    )?;
    Ok(())
}

fn apply_agent_session_custom_title_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSION_CUSTOM_TITLE],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "agent_sessions", "custom_title")? {
        connection
            .execute_batch("ALTER TABLE agent_sessions ADD COLUMN custom_title TEXT NULL;")?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_CUSTOM_TITLE, now_string()],
    )?;
    Ok(())
}

fn apply_agent_session_conversation_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_SESSION_CONVERSATION],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }
    if !column_exists(connection, "agent_sessions", "conversation_id")? {
        connection.execute_batch(
            "ALTER TABLE agent_sessions ADD COLUMN conversation_id TEXT NULL \
             REFERENCES conversations(id) ON DELETE SET NULL;",
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_SESSION_CONVERSATION, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_mode_agent_titles_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_MODE_AGENT_TITLES],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "conversations", "mode")? {
        connection.execute_batch(
            "ALTER TABLE conversations ADD COLUMN mode TEXT NOT NULL DEFAULT 'chat'
                 CHECK (mode IN ('chat', 'agent'));",
        )?;
    }
    for (column, definition) in [
        ("default_title", "TEXT NOT NULL DEFAULT ''"),
        ("generated_title", "TEXT NULL"),
        ("ai_vault_title", "TEXT NULL"),
        ("ai_vault_session_id", "TEXT NULL"),
        (
            "generation",
            "INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0)",
        ),
    ] {
        if !column_exists(connection, "agent_sessions", column)? {
            connection.execute_batch(&format!(
                "ALTER TABLE agent_sessions ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    connection.execute(
        "UPDATE conversations
         SET mode = 'agent'
         WHERE mode = 'chat'
           AND EXISTS (
             SELECT 1 FROM agent_sessions a
             WHERE a.conversation_id = conversations.id AND a.mode != 'gui'
           )",
        [],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_MODE_AGENT_TITLES, now_string()],
    )?;
    Ok(())
}

fn apply_credential_profiles_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CREDENTIAL_PROFILES],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(CREDENTIAL_PROFILES_SQL)?;
    if !column_exists(connection, "conversations", "credential_profile_id")? {
        connection.execute_batch(
            "ALTER TABLE conversations
             ADD COLUMN credential_profile_id TEXT
             REFERENCES credential_profiles(id) ON DELETE SET NULL;",
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_conversations_credential_profile_id
         ON conversations(credential_profile_id);",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CREDENTIAL_PROFILES, now_string()],
    )?;
    Ok(())
}

fn apply_usage_windows_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_USAGE_WINDOWS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(USAGE_WINDOWS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_USAGE_WINDOWS, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_archive_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_ARCHIVE],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "conversations", "archived_at")? {
        connection.execute_batch(CONVERSATION_ARCHIVE_SQL)?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_ARCHIVE, now_string()],
    )?;
    Ok(())
}

fn apply_agent_memories_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AGENT_MEMORIES],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(AGENT_MEMORIES_SQL)?;
    if !column_exists(connection, "conversations", "memories_mode")? {
        connection.execute_batch(
            "ALTER TABLE conversations ADD COLUMN memories_mode TEXT
                CHECK (memories_mode IN ('off', 'on'));",
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AGENT_MEMORIES, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_fast_mode_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_FAST_MODE],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "conversations", "fast_mode")? {
        connection.execute_batch(CONVERSATION_FAST_MODE_SQL)?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_FAST_MODE, now_string()],
    )?;
    Ok(())
}

fn apply_local_diagnostics_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_LOCAL_DIAGNOSTICS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(LOCAL_DIAGNOSTICS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_LOCAL_DIAGNOSTICS, now_string()],
    )?;
    Ok(())
}

fn apply_savings_report_queue_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SAVINGS_REPORT_QUEUE],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(SAVINGS_REPORT_QUEUE_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SAVINGS_REPORT_QUEUE, now_string()],
    )?;
    Ok(())
}

fn apply_savings_report_usd_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SAVINGS_REPORT_USD],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "savings_report_queue", "savings_usd")? {
        connection.execute(
            "ALTER TABLE savings_report_queue ADD COLUMN savings_usd REAL NULL",
            [],
        )?;
    }
    connection.execute(
        "UPDATE savings_report_queue
         SET savings_usd = COALESCE(
             (SELECT savings_usd
              FROM savings_snapshots
              WHERE message_id = savings_report_queue.message_id),
             (SELECT savings_usd
              FROM archived_savings_snapshots
              WHERE message_id = savings_report_queue.message_id),
             (SELECT savings_usd
              FROM agent_session_turns
              WHERE savings_report_queue.message_id =
                    'agent-turn:' || session_id || ':' || id),
             tokens_saved * ?1
         )
         WHERE savings_usd IS NULL",
        params![savings_usd_for_tokens(1)],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SAVINGS_REPORT_USD, now_string()],
    )?;
    Ok(())
}

fn apply_crux_index_state_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CRUX_INDEX_STATE],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(CRUX_INDEX_STATE_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CRUX_INDEX_STATE, now_string()],
    )?;
    Ok(())
}

fn apply_crux_savings_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CRUX_SAVINGS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "savings_snapshots", "crux_tokens_saved")? {
        connection.execute(
            "ALTER TABLE savings_snapshots
             ADD COLUMN crux_tokens_saved INTEGER NOT NULL DEFAULT 0
             CHECK (crux_tokens_saved >= 0)",
            [],
        )?;
    }
    if !column_exists(
        connection,
        "archived_savings_snapshots",
        "crux_tokens_saved",
    )? {
        connection.execute(
            "ALTER TABLE archived_savings_snapshots
             ADD COLUMN crux_tokens_saved INTEGER NOT NULL DEFAULT 0
             CHECK (crux_tokens_saved >= 0)",
            [],
        )?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CRUX_SAVINGS, now_string()],
    )?;
    Ok(())
}

fn record_local_activation(connection: &Connection) -> StorageResult<()> {
    let now = now_string();
    connection.execute(
        "INSERT INTO local_diagnostics (
             id, first_activated_at, last_activated_at, launch_count, first_launch_at
         ) VALUES (1, ?1, ?1, 1, ?1)
         ON CONFLICT(id) DO UPDATE SET
             first_launch_at = COALESCE(local_diagnostics.first_launch_at, excluded.first_launch_at),
             last_activated_at = excluded.last_activated_at,
            launch_count = local_diagnostics.launch_count + 1",
        params![now],
    )?;
    Ok(())
}

fn apply_turn_diffs_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TURN_DIFFS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS turn_snapshots (
            conversation_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            display_path TEXT NOT NULL,
            absolute_path TEXT NOT NULL,
            before BLOB NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (conversation_id, turn_id, absolute_path),
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS turn_diffs (
            conversation_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            diff TEXT NOT NULL,
            cwd TEXT NOT NULL,
            created_at TEXT NOT NULL,
            revertible INTEGER NOT NULL DEFAULT 1 CHECK (revertible IN (0, 1)),
            PRIMARY KEY (conversation_id, turn_id),
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_turn_snapshots_conversation_turn
            ON turn_snapshots(conversation_id, turn_id);
        CREATE INDEX IF NOT EXISTS idx_turn_diffs_conversation_created
            ON turn_diffs(conversation_id, created_at ASC, turn_id ASC);",
    )?;

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TURN_DIFFS, now_string()],
    )?;
    Ok(())
}

fn repair_legacy_turn_snapshots_primary_key(connection: &Connection) -> StorageResult<()> {
    // run_migrations calls this inside its immediate transaction and turns
    // foreign_keys OFF first when the legacy primary key exists.
    if !turn_snapshots_has_legacy_primary_key(connection)? {
        return Ok(());
    }
    rebuild_legacy_turn_snapshots(connection)
}

fn rebuild_legacy_turn_snapshots(connection: &Connection) -> StorageResult<()> {
    // The caller owns the transaction and the foreign_keys state.
    if !turn_snapshots_has_legacy_primary_key(connection)? {
        return Ok(());
    }

    let after_exists = if column_exists(connection, "turn_snapshots", "after_exists")? {
        "source.after_exists"
    } else {
        "NULL"
    };
    let after_sha256 = if column_exists(connection, "turn_snapshots", "after_sha256")? {
        "source.after_sha256"
    } else {
        "NULL"
    };

    connection.execute_batch(
        "CREATE TABLE turn_snapshots_new (
            conversation_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            display_path TEXT NOT NULL,
            absolute_path TEXT NOT NULL,
            before BLOB NULL,
            created_at TEXT NOT NULL,
            after_exists INTEGER NULL
                CHECK (after_exists IS NULL OR after_exists IN (0, 1)),
            after_sha256 TEXT NULL,
            PRIMARY KEY (conversation_id, turn_id, absolute_path),
            FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );",
    )?;
    connection.execute_batch(&format!(
        "INSERT INTO turn_snapshots_new
            (conversation_id, turn_id, display_path, absolute_path, before, created_at,
             after_exists, after_sha256)
         SELECT source.conversation_id, source.turn_id, source.display_path,
                source.absolute_path, source.before, source.created_at,
                {after_exists}, {after_sha256}
         FROM turn_snapshots AS source
         WHERE source.rowid = (
             SELECT MAX(candidate.rowid)
             FROM turn_snapshots AS candidate
             WHERE candidate.conversation_id = source.conversation_id
               AND candidate.turn_id = source.turn_id
               AND candidate.absolute_path = source.absolute_path
         );"
    ))?;
    connection.execute_batch(
        "DROP TABLE turn_snapshots;
         ALTER TABLE turn_snapshots_new RENAME TO turn_snapshots;
         CREATE INDEX idx_turn_snapshots_conversation_turn
             ON turn_snapshots(conversation_id, turn_id);",
    )?;
    Ok(())
}

fn turn_snapshots_has_legacy_primary_key(connection: &Connection) -> StorageResult<bool> {
    Ok(matches!(
        column_primary_key_position(connection, "turn_snapshots", "absolute_path")?,
        Some(0)
    ))
}

fn apply_turn_snapshot_postconditions_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_TURN_SNAPSHOT_POSTCONDITIONS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !column_exists(connection, "turn_snapshots", "after_exists")? {
        connection.execute_batch(
            "ALTER TABLE turn_snapshots
                 ADD COLUMN after_exists INTEGER NULL
                 CHECK (after_exists IS NULL OR after_exists IN (0, 1));",
        )?;
    }
    if !column_exists(connection, "turn_snapshots", "after_sha256")? {
        connection.execute_batch(
            "ALTER TABLE turn_snapshots
                 ADD COLUMN after_sha256 TEXT NULL;",
        )?;
    }
    if !column_exists(connection, "turn_diffs", "revertible")? {
        connection.execute_batch(
            "ALTER TABLE turn_diffs
                 ADD COLUMN revertible INTEGER NOT NULL DEFAULT 1
                 CHECK (revertible IN (0, 1));",
        )?;
    }

    connection.execute("UPDATE turn_diffs SET revertible = 0", [])?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_TURN_SNAPSHOT_POSTCONDITIONS, now_string()],
    )?;
    Ok(())
}

fn apply_message_mentions_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_MESSAGE_MENTIONS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !messages_mentions_column_exists(connection)? {
        connection.execute_batch("ALTER TABLE messages ADD COLUMN mentions TEXT;")?;
    }

    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_MESSAGE_MENTIONS, now_string()],
    )?;
    Ok(())
}

fn apply_auth_session_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_AUTH_SESSION],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(AUTH_SESSION_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_AUTH_SESSION, now_string()],
    )?;
    Ok(())
}

fn apply_search_and_titles_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SEARCH_AND_TITLES],
            |row| row.get(0),
        )
        .optional()?;

    if !conversation_title_source_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN title_source TEXT NOT NULL DEFAULT 'default'
                CHECK (title_source IN ('default', 'generated', 'manual'));",
        )?;
    }
    if !conversation_title_generation_attempted_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN title_generation_attempted INTEGER NOT NULL DEFAULT 0
                CHECK (title_generation_attempted IN (0, 1));",
        )?;
    }

    if fts5_available(connection)? {
        let had_fts_table = message_fts_table_exists(connection)?;
        connection.execute_batch(MESSAGE_SEARCH_FTS_SQL)?;
        if !had_fts_table {
            connection
                .execute_batch("INSERT INTO messages_fts(messages_fts) VALUES ('rebuild');")?;
        }
    }

    if already_applied.is_none() {
        connection.execute(
            "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
            params![MIGRATION_SEARCH_AND_TITLES, now_string()],
        )?;
    }
    Ok(())
}

fn apply_workspaces_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_WORKSPACES],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(WORKSPACES_SQL)?;
    if !project_id_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL;",
        )?;
    }
    if !workspace_id_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN workspace_id TEXT REFERENCES workspaces(id) ON DELETE SET NULL;",
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_conversations_workspace_id
            ON conversations(workspace_id);",
    )?;
    let now = now_string();
    connection.execute(
        "INSERT OR IGNORE INTO workspaces
            (id, project_id, name, kind, root_path, managed, pinned, state,
             created_at, last_used_at)
         SELECT 'primary-' || p.id, p.id, p.name, 'primary', p.root_path,
                0, 1, 'available', ?1, COALESCE(p.last_opened_at, p.created_at)
         FROM projects p",
        params![&now],
    )?;
    connection.execute(
        "UPDATE conversations
         SET workspace_id = 'primary-' || project_id
         WHERE project_id IS NOT NULL AND workspace_id IS NULL",
        [],
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_WORKSPACES, now],
    )?;
    Ok(())
}

fn apply_archived_savings_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_ARCHIVED_SAVINGS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(ARCHIVED_SAVINGS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_ARCHIVED_SAVINGS, now_string()],
    )?;
    Ok(())
}

fn apply_message_attachments_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_MESSAGE_ATTACHMENTS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(MESSAGE_ATTACHMENTS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_MESSAGE_ATTACHMENTS, now_string()],
    )?;
    Ok(())
}

fn apply_conversation_workflow_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_WORKFLOW],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    if !conversation_workflow_mode_column_exists(connection)? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN workflow_mode TEXT NOT NULL DEFAULT 'default'
                CHECK (workflow_mode IN ('default', 'plan'));",
        )?;
    }
    connection.execute_batch(CONVERSATION_WORKFLOW_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_WORKFLOW, now_string()],
    )?;
    Ok(())
}

fn apply_subagent_threads_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_SUBAGENT_THREADS],
            |row| row.get(0),
        )
        .optional()?;

    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(SUBAGENT_THREADS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SUBAGENT_THREADS, now_string()],
    )?;
    Ok(())
}

fn apply_workspace_local_environment_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_WORKSPACE_LOCAL_ENVIRONMENT],
            |row| row.get(0),
        )
        .optional()?;
    if !column_exists(connection, "workspaces", "local_environment_config_path")? {
        connection.execute_batch(
            "ALTER TABLE workspaces
                ADD COLUMN local_environment_config_path TEXT;",
        )?;
    }
    if already_applied.is_none() {
        connection.execute(
            "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
            params![MIGRATION_WORKSPACE_LOCAL_ENVIRONMENT, now_string()],
        )?;
    }
    Ok(())
}

fn apply_conversation_worktree_handoff_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_WORKTREE_HANDOFF],
            |row| row.get(0),
        )
        .optional()?;
    if !column_exists(connection, "conversations", "associated_worktree_id")? {
        connection.execute_batch(
            "ALTER TABLE conversations
                ADD COLUMN associated_worktree_id TEXT
                    REFERENCES workspaces(id) ON DELETE SET NULL;",
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_conversations_associated_worktree_id
            ON conversations(associated_worktree_id);",
    )?;
    connection.execute(
        "UPDATE conversations
         SET associated_worktree_id = workspace_id
         WHERE associated_worktree_id IS NULL
           AND workspace_id IN (
               SELECT id FROM workspaces WHERE kind = 'worktree'
           )",
        [],
    )?;
    if already_applied.is_none() {
        connection.execute(
            "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
            params![MIGRATION_CONVERSATION_WORKTREE_HANDOFF, now_string()],
        )?;
    }
    Ok(())
}

fn apply_conversation_compactions_migration(connection: &Connection) -> StorageResult<()> {
    let already_applied: Option<String> = connection
        .query_row(
            "SELECT version FROM migrations WHERE version = ?1",
            params![MIGRATION_CONVERSATION_COMPACTIONS],
            |row| row.get(0),
        )
        .optional()?;
    if already_applied.is_some() {
        return Ok(());
    }

    connection.execute_batch(CONVERSATION_COMPACTIONS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_CONVERSATION_COMPACTIONS, now_string()],
    )?;
    Ok(())
}

fn recover_interrupted_subagents(connection: &Connection) -> StorageResult<()> {
    connection.execute(
        "UPDATE subagent_threads
         SET status = 'interrupted',
             error = COALESCE(error, 'Halv restarted before this subagent finished.'),
             updated_at = ?1
         WHERE status IN ('pending', 'running')",
        params![now_string()],
    )?;
    Ok(())
}

fn project_id_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "project_id")
}

fn workspace_id_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "workspace_id")
}

#[cfg(test)]
fn associated_worktree_id_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "associated_worktree_id")
}

fn agent_mode_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "agent_mode")
}

fn conversation_workflow_mode_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "workflow_mode")
}

fn project_trust_level_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "projects", "trust_level")
}

fn project_approval_policy_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "projects", "approval_policy")
}

fn conversation_approval_policy_override_column_exists(
    connection: &Connection,
) -> StorageResult<bool> {
    column_exists(connection, "conversations", "approval_policy_override")
}

fn messages_mentions_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "messages", "mentions")
}

fn conversation_title_source_column_exists(connection: &Connection) -> StorageResult<bool> {
    column_exists(connection, "conversations", "title_source")
}

fn conversation_title_generation_attempted_column_exists(
    connection: &Connection,
) -> StorageResult<bool> {
    column_exists(connection, "conversations", "title_generation_attempted")
}

#[cfg(test)]
fn auth_session_table_exists(connection: &Connection) -> StorageResult<bool> {
    table_exists(connection, "auth_session")
}

fn message_fts_table_exists(connection: &Connection) -> StorageResult<bool> {
    table_exists(connection, "messages_fts")
}

fn table_exists(connection: &Connection, table: &str) -> StorageResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn column_exists(connection: &Connection, table: &str, name: &str) -> StorageResult<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&pragma)?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;

    for column in columns {
        if column? == name {
            return Ok(true);
        }
    }

    Ok(false)
}

fn column_primary_key_position(
    connection: &Connection,
    table: &str,
    name: &str,
) -> StorageResult<Option<i64>> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&pragma)?;
    let columns = statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
    })?;

    for column in columns {
        let (column_name, primary_key_position) = column?;
        if column_name == name {
            return Ok(Some(primary_key_position));
        }
    }

    Ok(None)
}

fn title_from_message(message: &str) -> String {
    let compact = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let compact = compact.trim();
    if compact.is_empty() {
        return "New conversation".to_string();
    }

    let mut title = compact.chars().take(72).collect::<String>();
    if compact.chars().count() > 72 {
        title.push_str("...");
    }
    title
}

fn title_from_goal(goal: &str) -> String {
    title_from_message(&format!("Goal: {goal}"))
}

fn fts5_available(connection: &Connection) -> StorageResult<bool> {
    match connection.execute_batch(
        "DROP TABLE IF EXISTS temp.halv_fts5_probe;
         CREATE VIRTUAL TABLE temp.halv_fts5_probe USING fts5(content);
         DROP TABLE temp.halv_fts5_probe;",
    ) {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(_, _)) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn fts_match_query(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(quote_fts_term)
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

fn quote_fts_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn search_messages_fts(
    connection: &Connection,
    match_query: &str,
) -> StorageResult<Vec<MessageSearchResult>> {
    let mut statement = connection.prepare(
        "SELECT
            c.id,
            c.title,
            m.id,
            m.role,
            snippet(messages_fts, 0, ?2, ?3, '...', 18) AS snippet,
            bm25(messages_fts) AS rank
         FROM messages_fts
         INNER JOIN messages m ON m.rowid = messages_fts.rowid
         INNER JOIN conversations c ON c.id = m.conversation_id
         WHERE messages_fts MATCH ?1
         ORDER BY rank ASC, m.created_at DESC, m.id DESC
         LIMIT ?4",
    )?;
    let rows = statement.query_map(
        params![
            match_query,
            SEARCH_HIGHLIGHT_START,
            SEARCH_HIGHLIGHT_END,
            SEARCH_LIMIT as i64
        ],
        message_search_result_from_row,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn search_messages_like(
    connection: &Connection,
    query: &str,
) -> StorageResult<Vec<MessageSearchResult>> {
    let escaped = escape_like_query(query);
    let pattern = format!("%{escaped}%");
    let mut statement = connection.prepare(
        "SELECT
            c.id,
            c.title,
            m.id,
            m.role,
            m.content
         FROM messages m
         INNER JOIN conversations c ON c.id = m.conversation_id
         WHERE m.content LIKE ?1 ESCAPE '\\'
         ORDER BY m.created_at DESC, m.id DESC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![pattern, SEARCH_LIMIT as i64], |row| {
        let content: String = row.get(4)?;
        Ok(MessageSearchResult {
            conversation_id: row.get(0)?,
            conversation_title: row.get(1)?,
            message_id: row.get(2)?,
            role: row.get(3)?,
            snippet: like_snippet(&content, query),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn message_search_result_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MessageSearchResult> {
    Ok(MessageSearchResult {
        conversation_id: row.get(0)?,
        conversation_title: row.get(1)?,
        message_id: row.get(2)?,
        role: row.get(3)?,
        snippet: row.get(4)?,
    })
}

fn escape_like_query(query: &str) -> String {
    let mut escaped = String::new();
    for character in query.chars() {
        match character {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn like_snippet(content: &str, query: &str) -> String {
    let lower_content = content.to_ascii_lowercase();
    let lower_query = query.to_ascii_lowercase();
    let Some(byte_index) = lower_content.find(&lower_query) else {
        return content.chars().take(160).collect();
    };
    let start = content[..byte_index]
        .char_indices()
        .rev()
        .nth(32)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let end = content[byte_index..]
        .char_indices()
        .nth(query.chars().count() + 64)
        .map(|(index, _)| byte_index + index)
        .unwrap_or(content.len());
    let match_end = byte_index + query.len();
    format!(
        "{}{}{}{}{}{}{}",
        if start > 0 { "..." } else { "" },
        &content[start..byte_index],
        SEARCH_HIGHLIGHT_START,
        &content[byte_index..match_end.min(content.len())],
        SEARCH_HIGHLIGHT_END,
        &content[match_end.min(content.len())..end],
        if end < content.len() { "..." } else { "" },
    )
}

fn render_conversation_markdown(
    conversation: &ConversationSummary,
    messages: &[StoredMessage],
) -> String {
    let mut output = String::new();
    output.push_str("# ");
    output.push_str(&conversation.title);
    output.push_str("\n\n");
    output.push_str(&format!("Conversation ID: `{}`\n\n", conversation.id));
    output.push_str(&format!("Model: `{}`\n\n", conversation.model));

    for message in messages {
        output.push_str("## ");
        output.push_str(markdown_role_label(&message.role));
        output.push_str("\n\n");
        output.push_str(message.content.trim());
        output.push_str("\n\n");
    }

    output
}

fn markdown_role_label(role: &str) -> &str {
    match role {
        "user" => "User",
        "assistant" => "Assistant",
        "developer" => "Developer",
        "system" => "System",
        _ => "Message",
    }
}

impl From<ConversationSummary> for ConversationExportMetadata {
    fn from(summary: ConversationSummary) -> Self {
        Self {
            id: summary.id,
            title: summary.title,
            model: summary.model,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
            goal: summary.goal,
            agent_mode: summary.agent_mode,
            workflow_mode: summary.workflow_mode,
            project_id: summary.project_id,
            project_name: summary.project_name,
            project_root_path: summary.project_root_path,
            project_trust_level: summary.project_trust_level,
            approval_policy_override: summary.approval_policy_override,
            message_count: summary.message_count,
        }
    }
}

fn normalize_goal(goal: Option<&str>) -> Option<String> {
    goal.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_workflow_mode(mode: &str) -> StorageResult<()> {
    match mode {
        "default" | "plan" => Ok(()),
        _ => Err(StorageError::Validation(
            "workflow mode must be `default` or `plan`".to_string(),
        )),
    }
}

fn validate_subagent_status(status: &str) -> StorageResult<()> {
    match status {
        "pending" | "running" | "completed" | "errored" | "interrupted" => Ok(()),
        _ => Err(StorageError::Validation(
            "subagent status is invalid".to_string(),
        )),
    }
}

fn validate_subagent_message_role(role: &str) -> StorageResult<()> {
    match role {
        "user" | "assistant" | "error" | "system" => Ok(()),
        _ => Err(StorageError::Validation(
            "subagent message role is invalid".to_string(),
        )),
    }
}

fn normalize_project_name(name: &str, root_path: &str) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }

    Path::new(root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(root_path)
        .to_string()
}

fn now_string() -> String {
    Utc::now().to_rfc3339()
}

fn trailing_week_cutoff(now: chrono::DateTime<Utc>) -> String {
    (now - Duration::days(7)).to_rfc3339()
}

fn encode_mentions(mentions: Option<&[String]>) -> StorageResult<Option<String>> {
    mentions
        .filter(|mentions| !mentions.is_empty())
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| {
            StorageError::Validation(format!("failed to encode message mentions: {error}"))
        })
}

fn decode_mentions(value: Option<String>, column: usize) -> rusqlite::Result<Option<Vec<String>>> {
    value
        .map(|value| {
            serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

pub(crate) fn new_uuid_v4() -> String {
    let mut bytes: [u8; 16] = random();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// The queue without the event times, which follow the clock.
    fn unreported_without_time(storage: &Storage) -> Vec<SavingsReportEvent> {
        storage
            .unreported_savings(500)
            .expect("queue should load")
            .into_iter()
            .map(|event| SavingsReportEvent {
                created_at: None,
                ..event
            })
            .collect()
    }

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-{name}-{}.sqlite3", new_uuid_v4()))
    }

    // rusqlite 0.40 only converts u64 and usize with its fallible_uint
    // feature. The conversions must work as in rusqlite 0.32: an INTEGER in
    // the u64 range reads as u64, a negative one fails, and a usize binds as
    // an INTEGER.
    #[test]
    fn unsigned_integers_convert_like_rusqlite_0_32() {
        let connection = Connection::open_in_memory().expect("database should open");
        connection
            .execute_batch("CREATE TABLE numbers (value INTEGER NOT NULL)")
            .expect("table should create");
        let index: usize = 7;
        connection
            .execute("INSERT INTO numbers (value) VALUES (?1)", params![index])
            .expect("usize should bind");
        connection
            .execute(
                "INSERT INTO numbers (value) VALUES (?1), (-1)",
                params![i64::MAX],
            )
            .expect("rows should insert");

        let read = |sql: &str| connection.query_row(sql, [], |row| row.get::<_, u64>(0));
        assert_eq!(
            read("SELECT value FROM numbers WHERE rowid = 1").unwrap(),
            7
        );
        assert_eq!(
            read("SELECT value FROM numbers WHERE rowid = 2").unwrap(),
            i64::MAX as u64
        );
        assert!(read("SELECT value FROM numbers WHERE rowid = 3").is_err());
        assert_eq!(read("SELECT COUNT(*) FROM numbers").unwrap(), 3);
        assert!(rusqlite::ToSql::to_sql(&u64::MAX).is_err());
    }

    #[test]
    fn storage_connection_uses_wal_journal_mode() {
        let path = test_db_path("journal-mode");
        let storage = Storage::open(&path).expect("open");
        let mode: String = storage
            .connection
            .lock()
            .expect("lock")
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal_mode");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal_mode was {mode}");
    }

    #[cfg(unix)]
    #[test]
    fn app_data_dir_and_database_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| {
            fs::metadata(path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
                .permissions()
                .mode()
                & 0o777
        };
        let directory = std::env::temp_dir().join(format!("halv-modes-{}", new_uuid_v4()));
        let files =
            ["", "-wal", "-shm"].map(|suffix| directory.join(format!("{DB_FILENAME}{suffix}")));

        let first = Storage::open_app_data_dir(&directory).expect("open");
        assert_eq!(mode(&directory), 0o700);
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }

        // An earlier release left the directory and the files readable by others.
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("chmod");
        for file in &files {
            fs::set_permissions(file, fs::Permissions::from_mode(0o644)).expect("chmod");
        }
        let second = Storage::open_app_data_dir(&directory).expect("reopen");
        assert_eq!(mode(&directory), 0o700);
        for file in &files {
            assert_eq!(mode(file), 0o600, "{}", file.display());
        }
        drop((first, second));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn saved_input_token_pricing_uses_fallback_rate() {
        assert_eq!(savings_usd_for_tokens(-1), 0.0);
        assert_eq!(savings_usd_for_tokens(0), 0.0);
        assert!((savings_usd_for_tokens(1_000_000) - 1.25).abs() < f64::EPSILON);
    }

    #[test]
    fn migrated_history_returns_the_next_turn_index() {
        let path = test_db_path("turn-index");
        let storage = Storage::open(&path).expect("open");
        storage
            .connection
            .lock()
            .expect("lock")
            .execute(
                "INSERT INTO agent_sessions
                   (id, agent_kind, lane, mode, cwd, status, created_at, last_active_at)
                 VALUES ('resumed', 'codex', 'codex_chatgpt', 'terminal_embedded',
                         '/tmp', 'running', '2026-08-19T11:00:00Z', '2026-08-19T11:00:00Z'),
                        ('previous', 'codex', 'codex_chatgpt', 'terminal_embedded',
                         '/tmp', 'exited', '2026-08-19T10:00:00Z', '2026-08-19T10:00:00Z')",
                [],
            )
            .expect("parent session rows");
        for index in 0..3 {
            storage
                .insert_agent_session_turn(&AgentSessionTurnRecord {
                    session_id: "previous",
                    turn_index: index,
                    input_tokens: 0,
                    output_tokens: 0,
                    ponytail_mode: None,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cost_usd: None,
                    savings_tokens: Some(10),
                    savings_usd: None,
                    pricing_source: None,
                    crux_tokens_saved: 0,
                    savings_source: Some("measured"),
                    has_message_snapshot: false,
                    created_at: "2026-08-19T12:00:00Z",
                })
                .expect("inherited rows insert");
        }
        let next_index = storage
            .migrate_agent_session_turns("previous", "resumed")
            .expect("turn migration");
        assert_eq!(next_index, 3);
        // The SQL guard must place a stale caller index after migrated rows.
        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "resumed",
                turn_index: 0,
                input_tokens: 0,
                output_tokens: 0,
                ponytail_mode: None,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(99),
                savings_usd: None,
                pricing_source: None,
                crux_tokens_saved: 0,
                savings_source: Some("measured"),
                has_message_snapshot: false,
                created_at: "2026-08-19T12:05:00Z",
            })
            .expect("post-resume sample must persist");
        let connection = storage.connection.lock().expect("lock");
        let (count, max_index, total): (i64, i64, i64) = connection
            .query_row(
                "SELECT COUNT(*), MAX(turn_index), SUM(savings_tokens)
                 FROM agent_session_turns WHERE session_id = 'resumed'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("query");
        assert_eq!((count, max_index, total), (4, 3, 129));
        drop(connection);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn inserted_agent_session_turns_return_increasing_row_ids() {
        let path = test_db_path("agent-turn-row-ids");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .connection
            .lock()
            .expect("storage lock should open")
            .execute(
                "INSERT INTO agent_sessions
                    (id, agent_kind, lane, mode, cwd, status, created_at, last_active_at)
                 VALUES ('row-id-session', 'codex', 'codex_chatgpt', 'terminal_embedded',
                    '/tmp', 'running', '2026-08-20T10:00:00Z', '2026-08-20T10:00:00Z')",
                [],
            )
            .expect("parent session should insert");

        let insert_turn = |turn_index| {
            storage.insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "row-id-session",
                turn_index,
                input_tokens: 0,
                output_tokens: 0,
                ponytail_mode: None,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: None,
                savings_usd: None,
                pricing_source: None,
                crux_tokens_saved: 0,
                savings_source: None,
                has_message_snapshot: false,
                created_at: "2026-08-20T10:01:00Z",
            })
        };
        let first_row_id = insert_turn(0).expect("first turn should insert");
        let second_row_id = insert_turn(1).expect("second turn should insert");

        assert_ne!(first_row_id, second_row_id);
        assert!(second_row_id > first_row_id);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    fn test_dir_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-{name}-{}", new_uuid_v4()))
    }

    fn test_credential_profile(
        id: &str,
        provider: &str,
        name: &str,
        priority: i64,
    ) -> CredentialProfile {
        CredentialProfile {
            id: id.to_string(),
            provider: provider.to_string(),
            name: name.to_string(),
            kind: "api-key".to_string(),
            priority,
            status: "active".to_string(),
            last_validated_at: None,
            metadata_json: None,
            created_at: "2000-01-01T00:00:00Z".to_string(),
            updated_at: "2000-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn credential_profiles_migration_applies_to_fresh_database() {
        let path = test_db_path("credential-profiles-migration");
        let storage = Storage::open(&path).expect("storage should open");
        let connection = storage.connection.lock().expect("storage lock poisoned");

        let migration_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_CREDENTIAL_PROFILES],
                |row| row.get(0),
            )
            .expect("credential profiles migration should query");
        assert_eq!(migration_count, 1);
        assert!(table_exists(&connection, "credential_profiles")
            .expect("credential profiles table should inspect"));
        assert!(
            column_exists(&connection, "conversations", "credential_profile_id")
                .expect("conversation credential profile column should inspect")
        );

        let index_exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM sqlite_master
                   WHERE type = 'index'
                     AND name = 'idx_conversations_credential_profile_id'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("credential profile index should inspect");
        assert!(index_exists);

        let table_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'credential_profiles'",
                [],
                |row| row.get(0),
            )
            .expect("credential profiles schema should query");
        assert!(!table_sql.to_ascii_lowercase().contains("secret"));

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn credential_profiles_migration_is_idempotent_on_reopen() {
        let path = test_db_path("credential-profiles-idempotent");
        let first = Storage::open(&path).expect("first open should migrate");
        drop(first);

        let second = Storage::open(&path).expect("second open should not rerun migration");
        let connection = second.connection.lock().expect("storage lock poisoned");
        let migration_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_CREDENTIAL_PROFILES],
                |row| row.get(0),
            )
            .expect("credential profiles migration should query");
        assert_eq!(migration_count, 1);

        drop(connection);
        drop(second);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn credential_profile_crud_roundtrip_preserves_order_and_timestamps() {
        let path = test_db_path("credential-profiles-crud");
        let storage = Storage::open(&path).expect("storage should open");
        let profiles = [
            test_credential_profile("openai-later", "openai", "Zebra", 20),
            test_credential_profile("anthropic-first", "anthropic", "Alpha", 10),
            test_credential_profile("openai-first", "openai", "Beta", 10),
        ];
        for profile in &profiles {
            storage
                .insert_credential_profile(profile)
                .expect("credential profile should insert");
        }

        let listed = storage
            .list_credential_profiles()
            .expect("credential profiles should list");
        let listed_ids = listed
            .iter()
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            listed_ids,
            vec!["anthropic-first", "openai-first", "openai-later"]
        );
        assert_eq!(
            storage
                .get_credential_profile("openai-later")
                .expect("credential profile should load"),
            Some(profiles[0].clone())
        );

        let renamed = storage
            .rename_credential_profile("openai-later", "Primary")
            .expect("credential profile should rename");
        assert_eq!(renamed.name, "Primary");
        assert_ne!(renamed.updated_at, profiles[0].updated_at);

        let disabled = storage
            .set_credential_profile_status("openai-later", "disabled")
            .expect("credential profile status should update");
        assert_eq!(disabled.status, "disabled");
        assert_ne!(disabled.updated_at, renamed.updated_at);

        let prioritized = storage
            .set_credential_profile_priority("openai-later", -5)
            .expect("credential profile priority should update");
        assert_eq!(prioritized.priority, -5);
        assert_ne!(prioritized.updated_at, disabled.updated_at);

        let validated = storage
            .set_credential_profile_validated(
                "openai-later",
                Some("2026-08-16T12:00:00Z"),
                Some(r#"{"accountLabel":"Team"}"#),
            )
            .expect("credential profile validation metadata should update");
        assert_eq!(
            validated.last_validated_at.as_deref(),
            Some("2026-08-16T12:00:00Z")
        );
        assert_eq!(
            validated.metadata_json.as_deref(),
            Some(r#"{"accountLabel":"Team"}"#)
        );
        assert_ne!(validated.updated_at, prioritized.updated_at);

        storage
            .delete_credential_profile("openai-later")
            .expect("credential profile should delete");
        assert!(storage
            .get_credential_profile("openai-later")
            .expect("deleted credential profile should query")
            .is_none());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn credential_profile_rejects_invalid_status() {
        let path = test_db_path("credential-profiles-status");
        let storage = Storage::open(&path).expect("storage should open");
        let mut invalid_profile = test_credential_profile("invalid-insert", "openai", "Invalid", 0);
        invalid_profile.status = "paused".to_string();
        assert!(matches!(
            storage.insert_credential_profile(&invalid_profile),
            Err(StorageError::Validation(_))
        ));
        assert!(storage
            .get_credential_profile("invalid-insert")
            .expect("rejected credential profile should query")
            .is_none());

        let profile = test_credential_profile("valid-profile", "openai", "Valid", 0);
        storage
            .insert_credential_profile(&profile)
            .expect("valid credential profile should insert");
        assert!(matches!(
            storage.set_credential_profile_status(&profile.id, "paused"),
            Err(StorageError::Validation(_))
        ));
        assert_eq!(
            storage
                .get_credential_profile(&profile.id)
                .expect("credential profile should query")
                .expect("credential profile should exist")
                .status,
            "active"
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_credential_profile_can_be_set_and_cleared() {
        let path = test_db_path("conversation-credential-profile");
        let storage = Storage::open(&path).expect("storage should open");
        let profile = test_credential_profile("profile", "openai", "Primary", 0);
        storage
            .insert_credential_profile(&profile)
            .expect("credential profile should insert");
        let conversation_id = storage
            .ensure_conversation(None, "Use a profile", "gpt-5.5", None)
            .expect("conversation should create");

        let pinned = storage
            .set_conversation_credential_profile(&conversation_id, Some(profile.id.clone()))
            .expect("credential profile should pin");
        assert_eq!(pinned.credential_profile_id.as_deref(), Some("profile"));
        assert_eq!(
            storage
                .conversation_credential_profile(&conversation_id)
                .expect("credential profile pin should read")
                .as_deref(),
            Some("profile")
        );
        let listed = storage
            .list_conversations()
            .expect("conversations should list");
        assert_eq!(listed[0].credential_profile_id.as_deref(), Some("profile"));

        let cleared = storage
            .set_conversation_credential_profile(&conversation_id, None)
            .expect("credential profile should clear");
        assert_eq!(cleared.credential_profile_id, None);
        assert_eq!(
            storage
                .conversation_credential_profile(&conversation_id)
                .expect("cleared credential profile pin should read"),
            None
        );
        assert!(matches!(
            storage.set_conversation_credential_profile(
                "missing-conversation",
                Some(profile.id.clone())
            ),
            Err(StorageError::Sql(rusqlite::Error::QueryReturnedNoRows))
        ));

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn deleting_credential_profile_clears_conversation_reference() {
        let path = test_db_path("credential-profile-delete");
        let storage = Storage::open(&path).expect("storage should open");
        let profile = test_credential_profile("profile", "openai", "Primary", 0);
        storage
            .insert_credential_profile(&profile)
            .expect("credential profile should insert");
        let conversation_id = storage
            .ensure_conversation(None, "Use a profile", "gpt-5.5", None)
            .expect("conversation should create");
        storage
            .set_conversation_credential_profile(&conversation_id, Some(profile.id.clone()))
            .expect("credential profile should pin");

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key setting should query");
        assert_eq!(foreign_keys, 1);
        drop(connection);

        storage
            .delete_credential_profile(&profile.id)
            .expect("credential profile should delete");
        let conversation = storage
            .conversation_summary(&conversation_id)
            .expect("conversation should load");
        assert_eq!(conversation.credential_profile_id, None);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn capacity_records_roundtrip_in_stable_order() {
        let path = test_db_path("capacity-records-roundtrip");
        let storage = Storage::open(&path).expect("storage should open");
        let windows = [
            UsageWindow {
                id: None,
                provider: "openai".to_string(),
                credential_profile_id: None,
                model: Some("gpt-5.5".to_string()),
                window_kind: "tokens".to_string(),
                limit_value: Some(20_000),
                remaining: Some(12_000),
                used: Some(8_000),
                resets_at: Some("2026-08-16T13:00:00Z".to_string()),
                retry_after_seconds: None,
                source: "headers".to_string(),
                confidence: "known".to_string(),
                captured_at: "2026-08-16T12:00:00Z".to_string(),
            },
            UsageWindow {
                id: None,
                provider: "anthropic".to_string(),
                credential_profile_id: None,
                model: Some("claude-sonnet-5".to_string()),
                window_kind: "requests".to_string(),
                limit_value: Some(100),
                remaining: Some(90),
                used: Some(10),
                resets_at: Some("2026-08-16T12:01:00Z".to_string()),
                retry_after_seconds: Some(2),
                source: "headers".to_string(),
                confidence: "known".to_string(),
                captured_at: "2026-08-16T12:00:01Z".to_string(),
            },
        ];
        for window in &windows {
            storage
                .upsert_usage_window(window)
                .expect("usage window should upsert");
        }
        let cooldowns = [
            ProfileCooldown {
                provider: "openai".to_string(),
                credential_profile_id: None,
                reason: "rate_limit".to_string(),
                next_attempt_at: Some("2026-08-16T12:02:00Z".to_string()),
                last_error: Some("OpenAI limit reached".to_string()),
                recorded_at: "2026-08-16T12:00:02Z".to_string(),
            },
            ProfileCooldown {
                provider: "anthropic".to_string(),
                credential_profile_id: None,
                reason: "rate_limit".to_string(),
                next_attempt_at: None,
                last_error: Some("Anthropic limit reached".to_string()),
                recorded_at: "2026-08-16T12:00:03Z".to_string(),
            },
        ];
        for cooldown in &cooldowns {
            storage
                .upsert_profile_cooldown(cooldown)
                .expect("profile cooldown should upsert");
        }

        let listed_windows = storage
            .list_usage_windows()
            .expect("usage windows should list");
        assert_eq!(listed_windows.len(), 2);
        assert_eq!(listed_windows[0].provider, "anthropic");
        assert_eq!(listed_windows[1].provider, "openai");
        assert!(listed_windows.iter().all(|window| window.id.is_some()));
        let listed_cooldowns = storage
            .list_profile_cooldowns()
            .expect("profile cooldowns should list");
        assert_eq!(
            listed_cooldowns,
            vec![cooldowns[1].clone(), cooldowns[0].clone()]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn capacity_upserts_replace_null_keys_without_duplicates() {
        let path = test_db_path("capacity-upsert-replace");
        let storage = Storage::open(&path).expect("storage should open");
        let mut window = UsageWindow {
            id: None,
            provider: "openai".to_string(),
            credential_profile_id: None,
            model: None,
            window_kind: "requests".to_string(),
            limit_value: Some(100),
            remaining: Some(80),
            used: Some(20),
            resets_at: None,
            retry_after_seconds: None,
            source: "headers".to_string(),
            confidence: "known".to_string(),
            captured_at: "2026-08-16T12:00:00Z".to_string(),
        };
        storage
            .upsert_usage_window(&window)
            .expect("first usage window should upsert");
        let first_id = storage
            .list_usage_windows()
            .expect("first usage window should list")[0]
            .id;
        window.remaining = Some(60);
        window.used = Some(40);
        window.captured_at = "2026-08-16T12:05:00Z".to_string();
        storage
            .upsert_usage_window(&window)
            .expect("latest usage window should replace first");
        let listed = storage
            .list_usage_windows()
            .expect("latest usage window should list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, first_id);
        assert_eq!(listed[0].remaining, Some(60));
        assert_eq!(listed[0].captured_at, "2026-08-16T12:05:00Z");

        let mut cooldown = ProfileCooldown {
            provider: "openai".to_string(),
            credential_profile_id: None,
            reason: "rate_limit".to_string(),
            next_attempt_at: None,
            last_error: Some("x".repeat(350)),
            recorded_at: "2026-08-16T12:00:00Z".to_string(),
        };
        storage
            .upsert_profile_cooldown(&cooldown)
            .expect("first cooldown should upsert");
        cooldown.reason = "usage_limit".to_string();
        cooldown.recorded_at = "2026-08-16T12:06:00Z".to_string();
        storage
            .upsert_profile_cooldown(&cooldown)
            .expect("latest cooldown should replace first");
        let listed = storage
            .list_profile_cooldowns()
            .expect("latest cooldown should list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, "usage_limit");
        assert_eq!(listed[0].last_error.as_deref().map(str::len), Some(300));

        storage
            .clear_profile_cooldown("openai", None)
            .expect("cooldown should clear");
        assert!(storage
            .list_profile_cooldowns()
            .expect("cleared cooldowns should list")
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn deleting_credential_profile_cascades_capacity_records() {
        let path = test_db_path("capacity-profile-cascade");
        let storage = Storage::open(&path).expect("storage should open");
        let profile = test_credential_profile("profile", "openai", "Primary", 0);
        storage
            .insert_credential_profile(&profile)
            .expect("credential profile should insert");
        storage
            .upsert_usage_window(&UsageWindow {
                id: None,
                provider: "openai".to_string(),
                credential_profile_id: Some(profile.id.clone()),
                model: Some("gpt-5.5".to_string()),
                window_kind: "requests".to_string(),
                limit_value: Some(100),
                remaining: Some(50),
                used: Some(50),
                resets_at: None,
                retry_after_seconds: None,
                source: "headers".to_string(),
                confidence: "known".to_string(),
                captured_at: "2026-08-16T12:00:00Z".to_string(),
            })
            .expect("usage window should upsert");
        storage
            .upsert_profile_cooldown(&ProfileCooldown {
                provider: "openai".to_string(),
                credential_profile_id: Some(profile.id.clone()),
                reason: "rate_limit".to_string(),
                next_attempt_at: None,
                last_error: Some("Limit reached".to_string()),
                recorded_at: "2026-08-16T12:00:00Z".to_string(),
            })
            .expect("profile cooldown should upsert");

        storage
            .delete_credential_profile(&profile.id)
            .expect("credential profile should delete");
        assert!(storage
            .list_usage_windows()
            .expect("usage windows should list")
            .is_empty());
        assert!(storage
            .list_profile_cooldowns()
            .expect("profile cooldowns should list")
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_sessions_migration_applies_to_fresh_in_memory_database() {
        let connection = Connection::open_in_memory().expect("connection should open");
        run_migrations(&connection).expect("migrations should apply");

        for table in ["agent_sessions", "agent_session_turns"] {
            let table_count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .expect("agent session table should query");
            assert_eq!(table_count, 1, "{table} should exist");
        }

        let mut session_statement = connection
            .prepare("PRAGMA table_info(agent_sessions)")
            .expect("session table info should prepare");
        let session_columns = session_statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("session table info should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("session columns should read");
        assert!(session_columns.iter().any(|column| column == "mode"));
        assert!(session_columns
            .iter()
            .any(|column| column == "custom_title"));
        assert!(session_columns
            .iter()
            .any(|column| column == "ponytail_mode"));
        assert!(session_columns.iter().any(|column| column == "rtk_db_path"));
        assert!(session_columns
            .iter()
            .any(|column| column == "crux_ledger_path"));
        let mode_default: Option<String> = connection
            .query_row(
                "SELECT dflt_value FROM pragma_table_info('agent_sessions') WHERE name = 'mode'",
                [],
                |row| row.get(0),
            )
            .expect("session mode default should query");
        assert_eq!(mode_default.as_deref(), Some("'gui'"));

        let mut statement = connection
            .prepare("PRAGMA table_info(agent_session_turns)")
            .expect("turn table info should prepare");
        let turn_columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("turn table info should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("turn columns should read");
        assert!(turn_columns.iter().any(|column| column == "savings_tokens"));
        assert!(turn_columns
            .iter()
            .any(|column| column == "crux_tokens_saved"));
        assert!(turn_columns.iter().any(|column| column == "savings_source"));
        assert!(turn_columns.iter().any(|column| column == "ponytail_mode"));
        assert!(turn_columns.iter().any(|column| column == "savings_usd"));
        assert!(turn_columns.iter().any(|column| column == "pricing_source"));
        assert!(turn_columns
            .iter()
            .any(|column| column == "has_message_snapshot"));
        assert!(turn_columns
            .iter()
            .any(|column| column == "occurred_local_date"));

        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSIONS],
                |row| row.get(0),
            )
            .expect("agent session migration marker should query");
        assert_eq!(marker_count, 1);

        let crux_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_CRUX_SAVINGS],
                |row| row.get(0),
            )
            .expect("Crux migration marker should query");
        assert_eq!(crux_marker_count, 1);

        let pricing_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_SAVINGS_USD_PRICING],
                |row| row.get(0),
            )
            .expect("USD pricing migration marker should query");
        assert_eq!(pricing_marker_count, 1);

        let message_snapshot_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_MESSAGE_SNAPSHOT],
                |row| row.get(0),
            )
            .expect("message snapshot migration marker should query");
        assert_eq!(message_snapshot_marker_count, 1);

        let local_date_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_SAVINGS_LOCAL_DATE],
                |row| row.get(0),
            )
            .expect("local date migration marker should query");
        assert_eq!(local_date_marker_count, 1);

        let savings_paths_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_SAVINGS_PATHS],
                |row| row.get(0),
            )
            .expect("session savings paths migration marker should query");
        assert_eq!(savings_paths_marker_count, 1);

        let title_marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_CUSTOM_TITLE],
                |row| row.get(0),
            )
            .expect("agent session title migration marker should query");
        assert_eq!(title_marker_count, 1);
    }

    #[test]
    fn savings_usd_migration_adds_columns_and_prices_existing_rows() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(
                "CREATE TABLE migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                 );
                 CREATE TABLE conversations (
                    id TEXT PRIMARY KEY,
                    model TEXT NOT NULL,
                    agent_backend TEXT,
                    agent_session_model TEXT
                 );
                 CREATE TABLE messages (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    model TEXT
                 );
                 CREATE TABLE savings_snapshots (
                    message_id TEXT PRIMARY KEY,
                    tokens_saved INTEGER,
                    savings_usd REAL
                 );
                 CREATE TABLE archived_savings_snapshots (
                    message_id TEXT PRIMARY KEY,
                    tokens_saved INTEGER,
                    savings_usd REAL
                 );
                 CREATE TABLE agent_sessions (
                    id TEXT PRIMARY KEY,
                    agent_kind TEXT NOT NULL,
                    conversation_id TEXT
                 );
                 CREATE TABLE agent_session_turns (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    savings_tokens INTEGER,
                    cost_usd REAL
                 );
                 INSERT INTO conversations
                    (id, model, agent_backend, agent_session_model)
                 VALUES ('conversation', 'gpt-5.6-sol', 'codex_session', 'gpt-5.6-sol');
                 INSERT INTO messages (id, conversation_id, model)
                 VALUES ('message', 'conversation', 'gpt-5.6-sol');
                 INSERT INTO savings_snapshots (message_id, tokens_saved, savings_usd)
                 VALUES ('message', 1000000, NULL);
                 INSERT INTO archived_savings_snapshots
                    (message_id, tokens_saved, savings_usd)
                 VALUES ('archived', 1000000, NULL);
                 INSERT INTO agent_sessions (id, agent_kind, conversation_id)
                 VALUES ('session', 'codex', 'conversation');
                 INSERT INTO agent_session_turns
                    (id, session_id, savings_tokens, cost_usd)
                 VALUES (1, 'session', 1000000, 99.0);",
            )
            .expect("legacy savings schema should create");

        apply_savings_usd_pricing_migration(&connection)
            .expect("USD pricing migration should apply");

        for table in [
            "agent_session_turns",
            "savings_snapshots",
            "archived_savings_snapshots",
        ] {
            assert!(column_exists(&connection, table, "pricing_source")
                .expect("pricing source column should inspect"));
        }
        assert!(
            column_exists(&connection, "agent_session_turns", "savings_usd")
                .expect("turn USD column should inspect")
        );
        let turn: (f64, String) = connection
            .query_row(
                "SELECT savings_usd, pricing_source FROM agent_session_turns WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated turn should query");
        assert!((turn.0 - 4.0).abs() < f64::EPSILON);
        assert_eq!(turn.1, "model_table");
        let snapshot: (f64, String) = connection
            .query_row(
                "SELECT savings_usd, pricing_source FROM savings_snapshots",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated snapshot should query");
        assert!((snapshot.0 - 4.0).abs() < f64::EPSILON);
        assert_eq!(snapshot.1, "model_table");
        let archived: (f64, String) = connection
            .query_row(
                "SELECT savings_usd, pricing_source FROM archived_savings_snapshots",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated archived snapshot should query");
        assert!((archived.0 - 1.25).abs() < f64::EPSILON);
        assert_eq!(archived.1, "fallback");
    }

    #[test]
    fn savings_local_date_migration_backfills_current_timezone_dates() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(
                "CREATE TABLE migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                 );
                 CREATE TABLE messages (
                    id TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL
                 );
                 CREATE TABLE savings_snapshots (
                    message_id TEXT PRIMARY KEY,
                    tokens_saved INTEGER
                 );
                 CREATE TABLE agent_session_turns (
                    id INTEGER PRIMARY KEY,
                    created_at TEXT NOT NULL
                 );
                 CREATE TABLE archived_savings_snapshots (
                    message_id TEXT PRIMARY KEY,
                    created_at TEXT NOT NULL
                 );
                 INSERT INTO messages (id, created_at)
                 VALUES ('message', '2026-08-24T23:30:00-03:00');
                 INSERT INTO savings_snapshots (message_id, tokens_saved)
                 VALUES ('message', 10);
                 INSERT INTO agent_session_turns (id, created_at)
                 VALUES (1, '2026-08-24T23:30:00-03:00');
                 INSERT INTO archived_savings_snapshots (message_id, created_at)
                 VALUES ('archived', '2026-08-24T23:30:00-03:00');",
            )
            .expect("legacy savings schema should create");
        let expected_date = connection
            .query_row(
                "SELECT date('2026-08-24T23:30:00-03:00', 'localtime')",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("expected local date should resolve");

        apply_savings_local_date_migration(&connection).expect("local date migration should apply");
        apply_savings_local_date_migration(&connection)
            .expect("local date migration should be idempotent");

        for table in [
            "agent_session_turns",
            "savings_snapshots",
            "archived_savings_snapshots",
        ] {
            assert!(column_exists(&connection, table, "occurred_local_date")
                .expect("local date column should inspect"));
            let actual: String = connection
                .query_row(
                    &format!("SELECT occurred_local_date FROM {table}"),
                    [],
                    |row| row.get(0),
                )
                .expect("backfilled local date should query");
            assert_eq!(actual, expected_date);
        }
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_SAVINGS_LOCAL_DATE],
                |row| row.get(0),
            )
            .expect("local date migration marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn agent_session_savings_paths_migration_adds_nullable_columns() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(
                "CREATE TABLE migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL
                 );
                 CREATE TABLE agent_sessions (id TEXT PRIMARY KEY);",
            )
            .expect("legacy agent session schema should create");

        apply_agent_session_savings_paths_migration(&connection)
            .expect("session savings paths migration should apply");
        apply_agent_session_savings_paths_migration(&connection)
            .expect("session savings paths migration should be idempotent");

        assert!(column_exists(&connection, "agent_sessions", "rtk_db_path")
            .expect("RTK path column should inspect"));
        assert!(
            column_exists(&connection, "agent_sessions", "crux_ledger_path")
                .expect("Crux path column should inspect")
        );
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_SAVINGS_PATHS],
                |row| row.get(0),
            )
            .expect("session savings paths marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn inserted_turn_at_local_2330_keeps_its_local_date() {
        let path = test_db_path("turn-local-date");
        let storage = Storage::open(&path).expect("storage should open");
        let local_date = Local::now().date_naive();
        let local_time = local_date
            .and_hms_opt(23, 30, 0)
            .and_then(|value| Local.from_local_datetime(&value).earliest())
            .expect("local test time should resolve");
        let created_at = local_time.to_rfc3339();
        storage
            .connection
            .lock()
            .expect("storage lock should open")
            .execute(
                "INSERT INTO agent_sessions
                    (id, agent_kind, lane, mode, cwd, status, created_at, last_active_at)
                 VALUES ('local-date-session', 'codex', 'codex_chatgpt',
                    'terminal_embedded', '/tmp', 'running', ?1, ?1)",
                params![&created_at],
            )
            .expect("parent session should insert");
        let row_id = storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "local-date-session",
                turn_index: 0,
                input_tokens: 0,
                output_tokens: 0,
                ponytail_mode: None,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(10),
                savings_usd: Some(0.01),
                pricing_source: Some("model_table"),
                crux_tokens_saved: 0,
                savings_source: Some("measured"),
                has_message_snapshot: false,
                created_at: &created_at,
            })
            .expect("turn should insert");
        let actual: String = storage
            .connection
            .lock()
            .expect("storage lock should open")
            .query_row(
                "SELECT occurred_local_date FROM agent_session_turns WHERE id = ?1",
                params![row_id],
                |row| row.get(0),
            )
            .expect("turn local date should query");
        assert_eq!(actual, local_date.format("%Y-%m-%d").to_string());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ponytail_mode_migration_applies_to_fresh_and_0040_databases() {
        let fresh = Connection::open_in_memory().expect("fresh connection should open");
        run_migrations(&fresh).expect("fresh migrations should apply");
        for table in ["agent_sessions", "agent_session_turns", "messages"] {
            assert!(column_exists(&fresh, table, "ponytail_mode")
                .expect("fresh Ponytail column should inspect"));
        }

        let at_0040 = Connection::open_in_memory().expect("0040 connection should open");
        at_0040
            .execute_batch(MIGRATIONS_SQL)
            .expect("migration table should create");
        at_0040
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("initial schema should create");
        at_0040
            .execute_batch(AGENT_SESSIONS_SQL)
            .expect("agent session schema should create");
        at_0040
            .execute(
                "INSERT INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_CONVERSATION_DRAFT, now_string()],
            )
            .expect("0040 marker should insert");

        apply_ponytail_mode_migration(&at_0040).expect("Ponytail migration should apply");
        for table in ["agent_sessions", "agent_session_turns", "messages"] {
            assert!(column_exists(&at_0040, table, "ponytail_mode")
                .expect("upgraded Ponytail column should inspect"));
        }
        let marker_count: i64 = at_0040
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_PONYTAIL_MODE],
                |row| row.get(0),
            )
            .expect("Ponytail migration marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn ponytail_mode_round_trips_on_turns_and_messages() {
        let path = test_db_path("ponytail-mode");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Ponytail", "gpt-5.5", None)
            .expect("conversation should create");
        let session_id = new_uuid_v4();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "gui",
                ponytail_mode: Some(PonytailMode::Full),
                project_id: None,
                conversation_id: Some(&conversation_id),
                cwd: "/tmp/project",
                cli_session_id: None,
                custom_title: None,
                default_title: "Codex 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "waiting_input",
                created_at: "2026-08-21T12:00:00Z",
                last_active_at: "2026-08-21T12:00:00Z",
                exit_code: None,
            })
            .expect("session should insert");
        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: &session_id,
                turn_index: 0,
                input_tokens: 10,
                output_tokens: 5,
                ponytail_mode: Some(PonytailMode::Full),
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: None,
                savings_usd: None,
                pricing_source: None,
                crux_tokens_saved: 0,
                savings_source: None,
                has_message_snapshot: false,
                created_at: "2026-08-21T12:01:00Z",
            })
            .expect("turn should insert");
        let message = storage
            .insert_message_with_ponytail(
                NewMessage {
                    conversation_id: &conversation_id,
                    role: "assistant",
                    content: "Done",
                    model: Some("gpt-5.5"),
                    input_tokens: Some(10),
                    output_tokens: Some(5),
                    error: false,
                    mentions: None,
                },
                Some(PonytailMode::Ultra),
            )
            .expect("message should insert");

        let connection = storage.connection.lock().expect("storage lock should open");
        let turn_mode: Option<String> = connection
            .query_row(
                "SELECT ponytail_mode FROM agent_session_turns WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .expect("turn mode should query");
        let message_mode: Option<String> = connection
            .query_row(
                "SELECT ponytail_mode FROM messages WHERE id = ?1",
                params![message.id],
                |row| row.get(0),
            )
            .expect("message mode should query");
        assert_eq!(
            turn_mode.map(|mode| mode.parse::<PonytailMode>().expect("valid turn mode")),
            Some(PonytailMode::Full)
        );
        assert_eq!(
            message_mode.map(|mode| mode.parse::<PonytailMode>().expect("valid message mode")),
            Some(PonytailMode::Ultra)
        );

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_conversation_migration_upgrades_existing_schema() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migration table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("conversation table should create");
        connection
            .execute_batch(
                "CREATE TABLE agent_sessions (
                   id TEXT PRIMARY KEY,
                   custom_title TEXT NULL
                 );",
            )
            .expect("old agent session table should create");

        apply_agent_session_conversation_migration(&connection)
            .expect("conversation migration should apply");

        assert!(
            column_exists(&connection, "agent_sessions", "conversation_id")
                .expect("conversation column should inspect")
        );
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_SESSION_CONVERSATION],
                |row| row.get(0),
            )
            .expect("conversation migration marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn conversation_mode_agent_titles_migration_backfills_terminal_conversations() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migration table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("conversation table should create");
        connection
            .execute_batch(
                "CREATE TABLE agent_sessions (
                   id TEXT PRIMARY KEY,
                   conversation_id TEXT NULL,
                   mode TEXT NOT NULL
                 );
                 INSERT INTO conversations
                   (id, title, model, created_at, updated_at)
                 VALUES
                   ('terminal-chat', 'Terminal', 'gpt-5.5', 'now', 'now'),
                   ('plain-chat', 'Chat', 'gpt-5.5', 'now', 'now');
                 INSERT INTO agent_sessions (id, conversation_id, mode)
                 VALUES ('terminal-session', 'terminal-chat', 'terminal_embedded');",
            )
            .expect("legacy data should create");

        apply_conversation_mode_agent_titles_migration(&connection)
            .expect("mode and title migration should apply");
        apply_conversation_mode_agent_titles_migration(&connection)
            .expect("mode and title migration should be idempotent");

        let mode = |id: &str| {
            connection
                .query_row(
                    "SELECT mode FROM conversations WHERE id = ?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .expect("conversation mode should query")
        };
        assert_eq!(mode("terminal-chat"), "agent");
        assert_eq!(mode("plain-chat"), "chat");
        for column in [
            "default_title",
            "generated_title",
            "ai_vault_title",
            "ai_vault_session_id",
            "generation",
        ] {
            assert!(column_exists(&connection, "agent_sessions", column)
                .expect("agent title column should inspect"));
        }
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_CONVERSATION_MODE_AGENT_TITLES],
                |row| row.get(0),
            )
            .expect("mode and title migration marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn agent_session_custom_title_round_trip_supports_reset() {
        let path = test_db_path("agent-session-custom-title");
        let storage = Storage::open(&path).expect("storage should open");
        let session_id = new_uuid_v4();
        let record = |custom_title| AgentSessionRecord {
            id: &session_id,
            agent_kind: "codex",
            lane: "codex_chatgpt",
            mode: "terminal_embedded",
            ponytail_mode: None,
            project_id: None,
            conversation_id: None,
            cwd: "/tmp/project",
            cli_session_id: Some("thread-123"),
            custom_title,
            default_title: "Claude 1",
            generated_title: None,
            ai_vault_title: None,
            ai_vault_session_id: None,
            generation: 0,
            status: "running",
            created_at: "2026-08-17T12:00:00Z",
            last_active_at: "2026-08-17T12:01:00Z",
            exit_code: None,
        };

        storage
            .upsert_agent_session(&record(Some("Release checks")))
            .expect("session title should insert");
        let read_title = || {
            storage
                .connection
                .lock()
                .expect("storage lock should not be poisoned")
                .query_row(
                    "SELECT custom_title FROM agent_sessions WHERE id = ?1",
                    params![session_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .expect("session title should query")
        };
        assert_eq!(read_title().as_deref(), Some("Release checks"));

        storage
            .upsert_agent_session(&record(None))
            .expect("session title should reset");
        assert_eq!(read_title(), None);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_headroom_route_survives_later_saves() {
        let path = test_db_path("agent-session-headroom-route");
        let storage = Storage::open(&path).expect("storage should open");
        let session_id = new_uuid_v4();
        let record = |status| AgentSessionRecord {
            id: &session_id,
            agent_kind: "claude",
            lane: "claude_api_key",
            mode: "terminal_embedded",
            ponytail_mode: None,
            project_id: None,
            conversation_id: None,
            cwd: "/tmp/project",
            cli_session_id: None,
            custom_title: None,
            default_title: "Claude 1",
            generated_title: None,
            ai_vault_title: None,
            ai_vault_session_id: None,
            generation: 0,
            status,
            created_at: "2026-10-22T12:00:00Z",
            last_active_at: "2026-10-22T12:00:00Z",
            exit_code: None,
        };
        let route = |storage: &Storage| {
            storage
                .get_agent_session(&session_id)
                .expect("agent session should load")
                .expect("agent session should exist")
                .headroom_route
        };
        storage
            .upsert_agent_session(&record("running"))
            .expect("agent session should insert");
        // A row without a saved route reads like a row from 1.0.0.
        assert_eq!(route(&storage), None);

        for saved in ["engine", "direct"] {
            storage
                .set_agent_session_headroom_route(&session_id, saved)
                .expect("the route should save");
            storage
                .upsert_agent_session(&record("waiting_input"))
                .expect("agent session should update");
            assert_eq!(route(&storage).as_deref(), Some(saved));
        }
        assert!(
            storage
                .set_agent_session_headroom_route(&session_id, "unknown")
                .is_err(),
            "the column accepts only the known routes"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_row_round_trip_and_stale_marking_preserve_history() {
        let path = test_db_path("agent-session-row");
        let storage = Storage::open(&path).expect("storage should open");
        let session_id = new_uuid_v4();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "zai",
                lane: "zai_glm",
                mode: "terminal_embedded",
                ponytail_mode: Some(PonytailMode::Full),
                project_id: None,
                conversation_id: None,
                cwd: "/tmp/project",
                cli_session_id: Some("cli-session-7"),
                custom_title: Some("Custom title"),
                default_title: "GLM 2",
                generated_title: Some("Generated title"),
                ai_vault_title: Some("Vault title"),
                ai_vault_session_id: Some("vault-session-4"),
                generation: 6,
                status: "running",
                created_at: "2026-08-24T12:00:00Z",
                last_active_at: "2026-08-24T12:01:00Z",
                exit_code: Some(9),
            })
            .expect("agent session should insert");
        let rtk_db_path = Path::new("/tmp/legacy/session-row.db");
        let crux_ledger_path = Path::new("/tmp/legacy/crux-row.jsonl");
        storage
            .update_agent_session_savings_paths(
                &session_id,
                Some(rtk_db_path),
                Some(crux_ledger_path),
            )
            .expect("agent session savings paths should update");

        let expected = AgentSessionRow {
            id: session_id.clone(),
            agent_kind: "zai".to_string(),
            lane: "zai_glm".to_string(),
            mode: "terminal_embedded".to_string(),
            ponytail_mode: Some("full".to_string()),
            project_id: None,
            conversation_id: None,
            cwd: "/tmp/project".to_string(),
            cli_session_id: Some("cli-session-7".to_string()),
            custom_title: Some("Custom title".to_string()),
            default_title: "GLM 2".to_string(),
            generated_title: Some("Generated title".to_string()),
            ai_vault_title: Some("Vault title".to_string()),
            ai_vault_session_id: Some("vault-session-4".to_string()),
            generation: 6,
            status: "running".to_string(),
            created_at: "2026-08-24T12:00:00Z".to_string(),
            last_active_at: "2026-08-24T12:01:00Z".to_string(),
            exit_code: Some(9),
            rtk_db_path: Some(rtk_db_path.to_string_lossy().into_owned()),
            crux_ledger_path: Some(crux_ledger_path.to_string_lossy().into_owned()),
            headroom_route: None,
        };
        assert_eq!(
            storage
                .get_agent_session(&session_id)
                .expect("agent session should load"),
            Some(expected.clone())
        );
        assert_eq!(
            storage
                .list_agent_sessions_with_status("running")
                .expect("running sessions should list"),
            vec![expected]
        );

        storage
            .reconcile_session_routes(&std::collections::HashSet::new())
            .expect("stale session should update");
        let exited = storage
            .get_agent_session(&session_id)
            .expect("agent session should reload")
            .expect("agent session should remain");
        assert_eq!(exited.status, "exited");
        assert_eq!(exited.exit_code, None);
        assert!(storage
            .list_agent_sessions_with_status("running")
            .expect("running sessions should relist")
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_layered_titles_and_generation_round_trip() {
        let path = test_db_path("agent-session-layered-title");
        let storage = Storage::open(&path).expect("storage should open");
        let session_id = new_uuid_v4();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "terminal_embedded",
                ponytail_mode: None,
                project_id: None,
                conversation_id: None,
                cwd: "/tmp/project",
                cli_session_id: Some("provider-session-7"),
                custom_title: Some("Ship blocker"),
                default_title: "Codex 3",
                generated_title: Some("Repair the release check"),
                ai_vault_title: Some("Release check repair"),
                ai_vault_session_id: Some("provider-session-7"),
                generation: 4,
                status: "waiting_input",
                created_at: "2026-08-17T12:00:00Z",
                last_active_at: "2026-08-17T12:01:00Z",
                exit_code: None,
            })
            .expect("layered title state should insert");

        let saved = storage
            .connection
            .lock()
            .expect("storage lock should not be poisoned")
            .query_row(
                "SELECT default_title, generated_title, ai_vault_title,
                        ai_vault_session_id, custom_title, generation
                 FROM agent_sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .expect("layered title state should query");
        assert_eq!(
            saved,
            (
                "Codex 3".to_string(),
                Some("Repair the release check".to_string()),
                Some("Release check repair".to_string()),
                Some("provider-session-7".to_string()),
                Some("Ship blocker".to_string()),
                4,
            )
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_conversation_binding_round_trips() {
        let path = test_db_path("agent-session-conversation");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Bound terminal", "gpt-5.5", None)
            .expect("conversation should create");
        let session_id = new_uuid_v4();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "terminal_embedded",
                ponytail_mode: None,
                project_id: None,
                conversation_id: Some(&conversation_id),
                cwd: "/tmp/project",
                cli_session_id: Some("thread-123"),
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: "2026-08-17T12:00:00Z",
                last_active_at: "2026-08-17T12:01:00Z",
                exit_code: None,
            })
            .expect("session should bind");

        let saved_conversation_id = storage
            .connection
            .lock()
            .expect("storage lock should not be poisoned")
            .query_row(
                "SELECT conversation_id FROM agent_sessions WHERE id = ?1",
                params![session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .expect("session conversation should query");
        assert_eq!(
            saved_conversation_id.as_deref(),
            Some(conversation_id.as_str())
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_agent_backend_migration_and_state_round_trip() {
        let path = test_db_path("conversation-agent-backend");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Claude route", "sonnet", None)
            .expect("conversation should create");

        assert_eq!(
            storage
                .conversation_agent_backend(&conversation_id)
                .expect("backend should read"),
            (None, None),
        );
        let summary = storage
            .set_conversation_agent_backend(
                &conversation_id,
                Some("claude_session"),
                Some("sonnet"),
            )
            .expect("backend should update");
        assert_eq!(summary.agent_backend.as_deref(), Some("claude_session"));
        assert_eq!(summary.agent_session_model.as_deref(), Some("sonnet"));
        for (backend, model) in [
            ("codex_session", "gpt-5.5"),
            ("kimi_session", "kimi-for-coding"),
            ("zai_session", "sonnet"),
            ("grok_session", "opus"),
        ] {
            let summary = storage
                .set_conversation_agent_backend(&conversation_id, Some(backend), Some(model))
                .expect("additional backend should update");
            assert_eq!(summary.agent_backend.as_deref(), Some(backend));
            assert_eq!(summary.agent_session_model.as_deref(), Some(model));
        }

        let connection = storage.connection.lock().expect("storage lock poisoned");
        assert!(column_exists(&connection, "conversations", "agent_backend")
            .expect("backend column should inspect"));
        assert!(
            column_exists(&connection, "conversations", "agent_session_model")
                .expect("model column should inspect")
        );
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_CONVERSATION_AGENT_BACKEND],
                |row| row.get(0),
            )
            .expect("migration marker should query");
        assert_eq!(marker_count, 1);
        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_mode_persists_and_rejects_invalid_values() {
        let path = test_db_path("conversation-mode");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Mode test", "gpt-5.5", None)
            .expect("conversation should create");

        assert_eq!(
            storage
                .conversation_summary(&conversation_id)
                .expect("conversation should load")
                .mode,
            "chat"
        );
        assert_eq!(
            storage
                .set_conversation_mode(&conversation_id, "agent")
                .expect("agent mode should save")
                .mode,
            "agent"
        );
        assert!(storage
            .set_conversation_mode(&conversation_id, "terminal")
            .is_err());

        drop(storage);
        let reopened = Storage::open(&path).expect("storage should reopen");
        assert_eq!(
            reopened
                .conversation_summary(&conversation_id)
                .expect("conversation should reload")
                .mode,
            "agent"
        );
        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_session_turn_insert_persists_measured_savings() {
        let storage =
            Storage::open(test_db_path("agent-turn-savings")).expect("storage should open");
        let session_id = new_uuid_v4();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: &session_id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "gui",
                ponytail_mode: None,
                project_id: None,
                conversation_id: None,
                cwd: "/tmp/project",
                cli_session_id: Some("thread-123"),
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "waiting_input",
                created_at: "2026-08-16T12:00:00Z",
                last_active_at: "2026-08-16T12:01:00Z",
                exit_code: None,
            })
            .expect("session should insert");
        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: &session_id,
                turn_index: 0,
                input_tokens: 100,
                output_tokens: 20,
                ponytail_mode: None,
                cache_read_tokens: 30,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(45),
                savings_usd: None,
                pricing_source: None,
                crux_tokens_saved: 0,
                savings_source: Some("measured"),
                has_message_snapshot: false,
                created_at: "2026-08-16T12:01:00Z",
            })
            .expect("turn should insert");

        let saved = storage
            .connection
            .lock()
            .expect("storage lock should not be poisoned")
            .query_row(
                "SELECT savings_tokens, savings_source
                 FROM agent_session_turns WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("turn savings should query");
        assert_eq!(saved, (45, "measured".to_string()));
    }

    #[test]
    fn agent_message_usage_round_trip_preserves_cache_and_source() {
        let path = test_db_path("agent-message-usage");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Session usage", "kimi-for-coding", None)
            .expect("conversation should create");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Done",
                model: Some("kimi-for-coding"),
                input_tokens: Some(100),
                output_tokens: Some(20),
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");
        storage
            .upsert_agent_message_usage(&assistant.id, 45, "usage")
            .expect("usage should insert");

        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should load");
        assert_eq!(messages[0].cached_tokens, Some(45));
        assert_eq!(messages[0].savings_source.as_deref(), Some("usage"));
        let marker_count: i64 = storage
            .connection
            .lock()
            .expect("storage lock poisoned")
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_AGENT_MESSAGE_USAGE],
                |row| row.get(0),
            )
            .expect("usage migration marker should query");
        assert_eq!(marker_count, 1);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn terminal_agent_savings_contribute_to_sidebar_totals_once() {
        let path = test_db_path("terminal-agent-savings-totals");
        let storage = Storage::open(&path).expect("storage should open");
        let created_at = now_string();
        storage
            .upsert_agent_session(&AgentSessionRecord {
                id: "terminal-session",
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "terminal_embedded",
                ponytail_mode: None,
                project_id: None,
                conversation_id: None,
                cwd: "/tmp",
                cli_session_id: Some("thread-1"),
                custom_title: None,
                default_title: "Claude 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "running",
                created_at: &created_at,
                last_active_at: &created_at,
                exit_code: None,
            })
            .expect("terminal session should insert");
        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "terminal-session",
                turn_index: 0,
                input_tokens: 0,
                output_tokens: 0,
                ponytail_mode: None,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(45),
                savings_usd: None,
                pricing_source: None,
                crux_tokens_saved: 0,
                savings_source: Some("measured"),
                has_message_snapshot: false,
                created_at: &created_at,
            })
            .expect("terminal savings should insert");

        let totals = storage.total_savings().expect("totals should load");
        assert_eq!(totals.tokens_saved, 45);
        assert_eq!(totals.session_tokens_saved, 45);
        let expected_usd = savings_usd_for_tokens(45);
        assert!((totals.savings_usd - expected_usd).abs() < f64::EPSILON);
        assert!((totals.session_savings_usd - expected_usd).abs() < f64::EPSILON);
        let today = storage
            .savings_since("0000-01-01T00:00:00Z")
            .expect("today should load");
        assert_eq!(today.tokens_saved, 45);
        assert!((today.savings_usd - expected_usd).abs() < f64::EPSILON);
        assert_eq!(today.recent_savings, vec![45]);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn gui_agent_aggregates_include_only_uncovered_turn_rows() {
        let path = test_db_path("gui-agent-savings-totals");
        let storage = Storage::open(&path).expect("storage should open");
        let fixture_date = Local::now().date_naive();
        let standalone_time = Local
            .from_local_datetime(
                &fixture_date
                    .and_hms_opt(12, 1, 0)
                    .expect("fixture time should be valid"),
            )
            .earliest()
            .expect("fixture time should resolve");
        let covered_time = standalone_time + Duration::minutes(1);
        let standalone_at = standalone_time.to_rfc3339();
        let covered_at = covered_time.to_rfc3339();
        let period_start = (standalone_time - Duration::hours(1)).to_rfc3339();
        let standalone_at = standalone_at.as_str();
        let covered_at = covered_at.as_str();
        let conversation_id = storage
            .ensure_conversation(None, "GUI savings", "gpt-5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Covered response",
                model: Some("gpt-5"),
                input_tokens: Some(100),
                output_tokens: Some(20),
                error: false,
                mentions: None,
            })
            .expect("assistant message should insert");
        storage
            .insert_savings_snapshot_with_crux(
                &assistant.id,
                Some(63),
                Some(0.063),
                Some(PricingSource::Headroom),
                7,
            )
            .expect("message snapshot should insert");
        storage
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                params![covered_at, &assistant.id],
            )
            .expect("message timestamp should update");
        storage
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE savings_snapshots
                 SET occurred_local_date = date(?1, 'localtime')
                 WHERE message_id = ?2",
                params![covered_at, &assistant.id],
            )
            .expect("snapshot local date should update");

        let insert_session = |id: &str, conversation_id: Option<&str>| {
            storage.upsert_agent_session(&AgentSessionRecord {
                id,
                agent_kind: "codex",
                lane: "codex_chatgpt",
                mode: "gui",
                ponytail_mode: None,
                project_id: None,
                conversation_id,
                cwd: "/tmp",
                cli_session_id: None,
                custom_title: None,
                default_title: "Codex 1",
                generated_title: None,
                ai_vault_title: None,
                ai_vault_session_id: None,
                generation: 0,
                status: "waiting_input",
                created_at: standalone_at,
                last_active_at: covered_at,
                exit_code: None,
            })
        };
        insert_session("gui-standalone", None).expect("standalone session should insert");
        insert_session("gui-covered", Some(&conversation_id))
            .expect("covered session should insert");

        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "gui-standalone",
                turn_index: 0,
                input_tokens: 10,
                output_tokens: 2,
                ponytail_mode: None,
                cache_read_tokens: 3,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(45),
                savings_usd: Some(0.045),
                pricing_source: Some(PricingSource::ModelTable.as_str()),
                crux_tokens_saved: 5,
                savings_source: Some("measured"),
                has_message_snapshot: false,
                created_at: standalone_at,
            })
            .expect("standalone turn should insert");
        storage
            .insert_agent_session_turn(&AgentSessionTurnRecord {
                session_id: "gui-covered",
                turn_index: 0,
                input_tokens: 100,
                output_tokens: 20,
                ponytail_mode: None,
                cache_read_tokens: 30,
                cache_creation_tokens: 0,
                cost_usd: None,
                savings_tokens: Some(70),
                savings_usd: Some(0.070),
                pricing_source: Some(PricingSource::Headroom.as_str()),
                crux_tokens_saved: 7,
                savings_source: Some("measured"),
                has_message_snapshot: true,
                created_at: covered_at,
            })
            .expect("covered turn should insert");

        let totals = storage.total_savings().expect("totals should load");
        assert_eq!(totals.tokens_saved, 115);
        assert_eq!(totals.crux_tokens_saved, 12);

        let period = storage
            .savings_since(&period_start)
            .expect("period savings should load");
        assert_eq!(period.tokens_saved, 115);
        assert_eq!(period.recent_savings, [45, 70]);

        let activity = storage
            .daily_token_activity(&period_start)
            .expect("daily activity should load");
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].input_tokens, 110);
        assert_eq!(activity[0].output_tokens, 22);
        assert_eq!(activity[0].tokens_saved, 115);

        let session_totals = storage
            .agent_session_turn_totals()
            .expect("session totals should load");
        assert_eq!(session_totals.len(), 1);
        assert_eq!(session_totals[0].session_id, "gui-standalone");
        assert_eq!(session_totals[0].savings_tokens, 45);
        assert_eq!(session_totals[0].crux_tokens_saved, 5);
        assert_eq!(session_totals[0].cache_read_tokens, 3);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn local_diagnostics_record_launches_and_only_return_aggregate_counts() {
        let path = test_db_path("local-diagnostics");
        let storage = Storage::open(&path).expect("storage should open");
        let first = storage
            .local_diagnostics()
            .expect("local diagnostics should query");
        assert!(first.healthy);
        assert_eq!(first.activation.launch_count, 1);
        assert_eq!(first.counts.conversations, 0);
        assert_eq!(first.counts.messages, 0);

        let project = storage
            .upsert_project("Private project name", "/Users/private/secret-project")
            .expect("project should insert");
        let conversation_id = storage
            .ensure_conversation(
                None,
                "Private conversation title",
                "gpt-5.5",
                Some(project.id),
            )
            .expect("conversation should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "A private prompt that diagnostics must never return",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("message should insert");
        storage
            .insert_agent_memories(
                Some(project.id),
                &["Private durable memory".to_string()],
                Some(&conversation_id),
            )
            .expect("memory should insert");

        let populated = storage
            .local_diagnostics()
            .expect("populated diagnostics should query");
        assert_eq!(populated.counts.conversations, 1);
        assert_eq!(populated.counts.messages, 1);
        assert_eq!(populated.counts.projects, 1);
        assert_eq!(populated.counts.workspaces, 1);
        assert_eq!(populated.counts.memories, 1);
        let serialized = serde_json::to_string(&populated).expect("diagnostics should serialize");
        assert!(!serialized.contains("Private"));
        assert!(!serialized.contains("secret-project"));

        let first_activated_at = populated.activation.first_activated_at;
        drop(storage);

        let reopened = Storage::open(&path).expect("storage should reopen");
        let second = reopened
            .local_diagnostics()
            .expect("reopened diagnostics should query");
        assert_eq!(second.activation.launch_count, 2);
        assert_eq!(second.activation.first_activated_at, first_activated_at);

        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn free_turn_migration_defaults_and_counter_persists_globally() {
        let path = test_db_path("free-turns");
        let connection = Connection::open(&path).expect("legacy database should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migration table should exist");
        connection
            .execute_batch(LOCAL_DIAGNOSTICS_SQL)
            .expect("legacy diagnostics table should exist");
        connection
            .execute(
                "INSERT INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_LOCAL_DIAGNOSTICS, now_string()],
            )
            .expect("legacy migration marker should insert");
        connection
            .execute(
                "INSERT INTO local_diagnostics
                    (id, first_activated_at, last_activated_at, launch_count)
                 VALUES (1, ?1, ?1, 1)",
                params![now_string()],
            )
            .expect("legacy diagnostics row should insert");
        drop(connection);

        let storage = Storage::open(&path).expect("free-turn migration should apply");
        assert_eq!(storage.free_turns_used().expect("counter should load"), 0);
        storage
            .connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE local_diagnostics SET free_turns_used = 2 WHERE id = 1",
                [],
            )
            .expect("legacy counter should update");

        let conversation_id = storage
            .ensure_conversation(None, "Temporary conversation", "gpt-5.5", None)
            .expect("conversation should insert");
        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");
        assert_eq!(storage.free_turns_used().expect("counter should remain"), 2);
        drop(storage);

        let reopened = Storage::open(&path).expect("storage should reopen");
        assert_eq!(
            reopened
                .free_turns_used()
                .expect("persisted counter should load"),
            2
        );
        let connection = reopened.connection.lock().expect("storage lock poisoned");
        assert!(
            column_exists(&connection, "local_diagnostics", "free_turns_used")
                .expect("free-turn column should inspect")
        );
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_FREE_TURNS],
                |row| row.get(0),
            )
            .expect("free-turn migration marker should query");
        assert_eq!(marker_count, 1);

        drop(connection);
        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn first_launch_migration_backfills_and_preserves_timestamp() {
        let path = test_db_path("first-launch");
        let first_launch_at = "2026-08-01T12:00:00Z";
        let connection = Connection::open(&path).expect("legacy database should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migration table should exist");
        connection
            .execute_batch(LOCAL_DIAGNOSTICS_SQL)
            .expect("legacy diagnostics table should exist");
        connection
            .execute(
                "INSERT INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_LOCAL_DIAGNOSTICS, now_string()],
            )
            .expect("legacy migration marker should insert");
        connection
            .execute(
                "INSERT INTO local_diagnostics
                 (id, first_activated_at, last_activated_at, launch_count)
                 VALUES (1, ?1, ?1, 1)",
                params![first_launch_at],
            )
            .expect("legacy diagnostics row should insert");
        drop(connection);

        let first = Storage::open(&path).expect("first-launch migration should apply");
        assert_eq!(
            first.first_launch_at().expect("first launch should load"),
            first_launch_at
        );
        drop(first);

        let reopened = Storage::open(&path).expect("storage should reopen");
        assert_eq!(
            reopened
                .first_launch_at()
                .expect("first launch should remain"),
            first_launch_at
        );
        let connection = reopened.connection.lock().expect("storage lock poisoned");
        assert!(
            column_exists(&connection, "local_diagnostics", "first_launch_at")
                .expect("first-launch column should inspect")
        );
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_FIRST_LAUNCH_AT],
                |row| row.get(0),
            )
            .expect("first-launch migration marker should query");
        assert_eq!(marker_count, 1);

        drop(connection);
        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_round_trip_create_insert_list_get() {
        let path = test_db_path("round-trip");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Hello from storage", "gpt-5.5", None)
            .expect("conversation should insert");
        let user_mentions = vec!["src/main.rs".to_string(), "README.md".to_string()];
        let user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Hello from storage",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: Some(&user_mentions),
            })
            .expect("user message should insert");
        assert_eq!(user.role, "user");

        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Hello back",
                model: Some("gpt-5.5"),
                input_tokens: Some(12),
                output_tokens: Some(7),
                error: false,
                mentions: None,
            })
            .expect("assistant message should insert");
        assert!(user.sequence < assistant.sequence);
        storage
            .insert_savings_snapshot(&assistant.id, Some(42), Some(0.25))
            .expect("savings snapshot should insert");

        let conversations = storage
            .list_conversations()
            .expect("conversations should list");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].id, conversation_id);
        assert_eq!(conversations[0].goal, None);
        assert_eq!(conversations[0].message_count, 2);

        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should list");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sequence, user.sequence);
        assert_eq!(messages[1].sequence, assistant.sequence);
        assert_eq!(messages[0].content, "Hello from storage");
        assert_eq!(
            messages[0].mentions.as_deref(),
            Some(user_mentions.as_slice())
        );
        assert_eq!(messages[1].content, "Hello back");
        assert_eq!(messages[1].mentions, None);
        assert_eq!(messages[1].input_tokens, Some(12));
        assert_eq!(messages[1].output_tokens, Some(7));
        assert_eq!(messages[1].tokens_saved, Some(42));
        assert_eq!(messages[1].savings_usd, Some(0.25));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn report_event_parts_split_at_the_per_event_limit() {
        assert_eq!(
            report_event_parts("small", 10),
            vec![ReportEventPart {
                id: "small".to_string(),
                tokens_saved: 10,
                share: 1.0,
            }]
        );
        let parts = report_event_parts("big", 25_000_000);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.tokens_saved)
                .collect::<Vec<_>>(),
            vec![10_000_000, 10_000_000, 5_000_000]
        );
        assert_eq!(
            parts
                .iter()
                .map(|part| part.id.as_str())
                .collect::<Vec<_>>(),
            vec!["big", "big#part2", "big#part3"]
        );
        let total_share: f64 = parts.iter().map(|part| part.share).sum();
        assert!((total_share - 1.0).abs() < 1e-12);
    }

    #[test]
    fn an_oversized_sample_splits_and_the_queue_advances_past_it() {
        let path = test_db_path("oversized-savings-report");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .enqueue_savings_report("big", 12_000_000, 120.0)
            .expect("big sample should enqueue");
        storage
            .enqueue_savings_report("small-1", 10, 0.1)
            .expect("small sample should enqueue");
        storage
            .enqueue_priced_savings_report(
                "small-2",
                20,
                &savings_dollars::ReportDollars {
                    kind: "context".to_string(),
                    net_usd: 0.2,
                    method: "effective_rate".to_string(),
                    price_version: "2026-10-15".to_string(),
                },
            )
            .expect("priced sample should enqueue");

        let queued = storage.unreported_savings(500).expect("queue should load");
        let ids: Vec<&str> = queued.iter().map(|event| event.id.as_str()).collect();
        assert_eq!(ids, vec!["big", "big#part2", "small-1", "small-2"]);
        assert!(queued
            .iter()
            .all(|event| event.tokens_saved <= MAX_TOKENS_SAVED_PER_REPORT_EVENT));
        assert_eq!(queued[0].tokens_saved, 10_000_000);
        assert_eq!(queued[1].tokens_saved, 2_000_000);
        assert!((queued[0].savings_usd - 100.0).abs() < 1e-9);
        assert!((queued[1].savings_usd - 20.0).abs() < 1e-9);
        assert!(queued.iter().all(|event| event.created_at.is_some()));
        let totals = storage.total_savings().expect("totals should load");
        assert_eq!(totals.pending_tokens_saved, 12_000_030);

        // A smaller sample with the same id drops the stale extra part.
        storage
            .enqueue_savings_report("big", 5, 0.05)
            .expect("smaller sample should update");
        let ids: Vec<String> = storage
            .unreported_savings(500)
            .expect("queue should load")
            .into_iter()
            .map(|event| event.id)
            .collect();
        assert_eq!(ids, vec!["big", "big#part2", "small-1", "small-2"]);
        storage
            .enqueue_savings_report("big", 12_000_000, 120.0)
            .expect("big sample should enqueue again");

        // The server stores the parts and refuses nothing; the queue empties.
        let delivered: Vec<String> = storage
            .unreported_savings(500)
            .expect("queue should load")
            .into_iter()
            .map(|event| event.id)
            .collect();
        storage
            .mark_savings_reported(&delivered)
            .expect("parts should mark reported");
        assert!(storage
            .unreported_savings(500)
            .expect("queue should load")
            .is_empty());

        // A dead letter leaves the queue and shows in the counts.
        storage
            .enqueue_savings_report("legacy-big", 10, 0.1)
            .expect("sample should enqueue");
        storage
            .mark_savings_rejected(&[("legacy-big".to_string(), "refused".to_string())])
            .expect("dead letter should save");
        assert_eq!(
            storage.savings_queue_counts().expect("counts should load"),
            SavingsQueueCounts {
                unreported: 0,
                rejected: 1,
            }
        );
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn enqueue_savings_report_is_idempotent_and_preserves_reported_rows() {
        let path = test_db_path("enqueue-savings-report");
        let storage = Storage::open(&path).expect("storage should open");
        let event_id = "agent-turn:session-id:7".to_string();

        storage
            .enqueue_savings_report(&event_id, 40, 0.40)
            .expect("savings should enqueue");
        storage
            .enqueue_savings_report(&event_id, 55, 0.55)
            .expect("unreported savings should update");
        assert_eq!(
            unreported_without_time(&storage),
            vec![SavingsReportEvent {
                id: event_id.clone(),
                tokens_saved: 55,
                savings_usd: 0.55,
                dollars: None,
                created_at: None,
            }]
        );

        storage
            .mark_savings_reported(std::slice::from_ref(&event_id))
            .expect("savings should mark reported");
        storage
            .enqueue_savings_report(&event_id, 99, 0.99)
            .expect("reported savings should stay unchanged");
        storage
            .enqueue_savings_report("agent-turn:session-id:8", 0, 0.0)
            .expect("zero savings should not enqueue");
        assert!(storage
            .unreported_savings(500)
            .expect("queue should load")
            .is_empty());

        let connection = storage.connection.lock().expect("storage lock should open");
        let (tokens_saved, savings_usd, reported_at): (i64, f64, Option<String>) = connection
            .query_row(
                "SELECT tokens_saved, savings_usd, reported_at
                 FROM savings_report_queue
                 WHERE message_id = ?1",
                params![event_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("reported row should load");
        let row_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM savings_report_queue", [], |row| {
                row.get(0)
            })
            .expect("queue count should load");
        assert_eq!(tokens_saved, 55);
        assert_eq!(savings_usd, 0.55);
        assert!(reported_at.is_some());
        assert_eq!(row_count, 1);

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn savings_report_usd_migration_backfills_pending_snapshot() {
        let path = test_db_path("savings-report-usd-migration");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Backfill savings", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Saved tokens",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");
        storage
            .insert_savings_snapshot(&assistant.id, Some(42), Some(0.25))
            .expect("savings snapshot should insert");

        let connection = storage.connection.lock().expect("storage lock should open");
        connection
            .execute(
                "UPDATE savings_report_queue SET savings_usd = NULL WHERE message_id = ?1",
                params![&assistant.id],
            )
            .expect("legacy queue row should update");
        connection
            .execute(
                "DELETE FROM migrations WHERE version = ?1",
                params![MIGRATION_SAVINGS_REPORT_USD],
            )
            .expect("migration marker should delete");
        apply_savings_report_usd_migration(&connection).expect("migration should reapply");
        let savings_usd: f64 = connection
            .query_row(
                "SELECT savings_usd FROM savings_report_queue WHERE message_id = ?1",
                params![assistant.id],
                |row| row.get(0),
            )
            .expect("backfilled USD should load");
        assert_eq!(savings_usd, 0.25);

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn positive_savings_are_queued_until_reported() {
        let path = test_db_path("savings-report-queue");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Report savings", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Saved tokens",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");

        storage
            .insert_savings_snapshot(&assistant.id, Some(0), Some(0.0))
            .expect("zero savings should insert without queueing");
        assert!(storage
            .unreported_savings(500)
            .expect("queue should load")
            .is_empty());

        storage
            .insert_savings_snapshot(&assistant.id, Some(42), Some(0.25))
            .expect("positive savings should enqueue");
        assert_eq!(
            unreported_without_time(&storage),
            vec![SavingsReportEvent {
                id: assistant.id.clone(),
                tokens_saved: 42,
                savings_usd: 0.25,
                dollars: None,
                created_at: None,
            }]
        );
        let pending = storage.total_savings().expect("pending totals should load");
        assert_eq!(pending.pending_tokens_saved, 42);
        assert_eq!(pending.pending_savings_usd, 0.25);

        storage
            .insert_savings_snapshot(&assistant.id, Some(55), Some(0.30))
            .expect("unreported savings should update");
        assert_eq!(
            unreported_without_time(&storage),
            vec![SavingsReportEvent {
                id: assistant.id.clone(),
                tokens_saved: 55,
                savings_usd: 0.30,
                dollars: None,
                created_at: None,
            }]
        );

        storage
            .mark_savings_reported(std::slice::from_ref(&assistant.id))
            .expect("queue row should mark reported");
        assert!(storage
            .unreported_savings(500)
            .expect("queue should load")
            .is_empty());
        let reported = storage
            .total_savings()
            .expect("reported totals should load");
        assert_eq!(reported.pending_tokens_saved, 0);
        assert_eq!(reported.pending_savings_usd, 0.0);

        storage
            .insert_savings_snapshot(&assistant.id, Some(99), Some(0.50))
            .expect("reported events should remain idempotent");
        assert!(storage
            .unreported_savings(500)
            .expect("reported event should stay hidden")
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn crux_savings_recording_is_idempotent_per_message() {
        let path = test_db_path("crux-savings-idempotent");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Crux savings", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Mapped the code",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");

        for _ in 0..2 {
            storage
                .insert_savings_snapshot_with_crux(
                    &assistant.id,
                    Some(42),
                    Some(0.25),
                    Some(PricingSource::Headroom),
                    5_200,
                )
                .expect("Crux savings should record");
        }

        let expected_usd = 0.25 + pricing::input_price(None, Some("gpt-5.5")).savings_usd(5_200);
        assert_eq!(
            unreported_without_time(&storage),
            vec![SavingsReportEvent {
                id: assistant.id.clone(),
                tokens_saved: 5_242,
                savings_usd: expected_usd,
                dollars: None,
                created_at: None,
            }]
        );
        let totals = storage.total_savings().expect("savings should total");
        assert_eq!(totals.tokens_saved, 5_242);
        assert!((totals.savings_usd - expected_usd).abs() < 0.000_000_001);
        assert_eq!(totals.crux_tokens_saved, 5_200);

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");
        let archived = storage
            .total_savings()
            .expect("archived savings should total");
        assert_eq!(archived.tokens_saved, 5_242);
        assert!((archived.savings_usd - expected_usd).abs() < 0.000_000_001);
        assert_eq!(archived.crux_tokens_saved, 5_200);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn savings_report_queue_survives_conversation_deletion() {
        let path = test_db_path("savings-report-survives-delete");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Delete after settle", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Saved tokens",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("assistant should insert");
        storage
            .insert_savings_snapshot(&assistant.id, Some(21), Some(0.1))
            .expect("positive savings should enqueue");

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");
        assert_eq!(
            unreported_without_time(&storage),
            vec![SavingsReportEvent {
                id: assistant.id,
                tokens_saved: 21,
                savings_usd: 0.1,
                dollars: None,
                created_at: None,
            }]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_archive_round_trips_and_preserves_messages() {
        let path = test_db_path("conversation-archive");
        let storage = Storage::open(&path).expect("storage should open");
        let archived_id = storage
            .ensure_conversation(None, "Archive me", "gpt-5.5", None)
            .expect("archived conversation should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &archived_id,
                role: "user",
                content: "Keep this message",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("message should insert");
        let active_id = storage
            .ensure_conversation(None, "Keep active", "gpt-5.5", None)
            .expect("active conversation should insert");

        let archived = storage
            .set_conversation_archived(&archived_id, true)
            .expect("conversation should archive");
        assert!(archived.archived_at.is_some());
        let conversations = storage
            .list_conversations()
            .expect("conversations should list");
        assert_eq!(conversations[0].id, active_id);
        assert_eq!(conversations[0].archived_at, None);
        assert_eq!(conversations[1].id, archived_id);
        assert!(conversations[1].archived_at.is_some());
        assert_eq!(
            storage
                .get_messages(&archived_id)
                .expect("archived messages should list")[0]
                .content,
            "Keep this message"
        );

        drop(storage);
        let reopened = Storage::open(&path).expect("storage should reopen");
        assert!(reopened
            .conversation_summary(&archived_id)
            .expect("archived summary should load")
            .archived_at
            .is_some());
        assert_eq!(
            reopened
                .set_conversation_archived(&archived_id, false)
                .expect("conversation should unarchive")
                .archived_at,
            None
        );

        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn memory_history_only_selects_matching_subscription_lanes() {
        let path = test_db_path("memory-subscription-lanes");
        let storage = Storage::open(&path).unwrap();
        for (id, kind, lane) in [
            ("codex", "codex", "codex_chatgpt"),
            ("claude", "claude", "claude_subscription"),
            ("other", "claude", "grok"),
            ("mismatched", "claude", "codex_chatgpt"),
        ] {
            storage.connection.lock().unwrap().execute(
                "INSERT INTO agent_sessions (id, agent_kind, lane, mode, cwd, cli_session_id, status, created_at, last_active_at) VALUES (?1, ?2, ?3, 'terminal_embedded', '/tmp/project', ?1, 'exited', '2026-09-09T12:00:00Z', '2026-09-09T12:00:00Z')",
                params![id, kind, lane],
            ).unwrap();
        }
        assert!(storage.agent_memory_subscription_session("codex").unwrap());
        assert!(storage.agent_memory_subscription_session("claude").unwrap());
        assert!(!storage.agent_memory_subscription_session("other").unwrap());
        assert!(!storage
            .agent_memory_subscription_session("mismatched")
            .unwrap());
        assert!(!storage
            .agent_memory_subscription_session("missing")
            .unwrap());
        let mut ids = storage
            .recent_memory_agent_sessions()
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, ["claude", "codex"]);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn memory_turn_checkpoint_survives_reopening() {
        let path = test_db_path("memory-turn-checkpoint");
        let storage = Storage::open(&path).unwrap();
        // Manual extraction can mark a turn before any background lookup.
        storage
            .mark_agent_memory_turn_processed("session:turn-1")
            .unwrap();
        storage
            .mark_agent_memory_turn_processed("session:turn-1")
            .unwrap();
        drop(storage);
        let storage = Storage::open(&path).unwrap();
        assert!(storage
            .agent_memory_turn_processed("session:turn-1")
            .unwrap());
        assert!(!storage
            .agent_memory_turn_processed("session:turn-2")
            .unwrap());
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_memories_round_trip_with_conversation_mode_controls() {
        let path = test_db_path("agent-memories-round-trip");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Remember this", "gpt-5.5", None)
            .expect("conversation should insert");
        let contents = vec![
            "The user prefers compact status updates.".to_string(),
            "Use pnpm for this workspace.".to_string(),
        ];

        assert_eq!(
            storage
                .insert_agent_memories(None, &contents, Some(&conversation_id))
                .expect("memories should insert"),
            2
        );
        let memories = storage.list_agent_memories().expect("memories should list");
        assert_eq!(memories.len(), 2);
        assert!(memories.iter().all(|memory| {
            memory.project_id.is_none()
                && memory.source_conversation_id.as_deref() == Some(&conversation_id)
        }));

        assert_eq!(
            storage
                .set_conversation_memories_mode(&conversation_id, Some("off"))
                .expect("memories mode should update")
                .memories_mode
                .as_deref(),
            Some("off")
        );
        assert_eq!(
            storage
                .conversation_memories_mode(&conversation_id)
                .expect("memories mode should load")
                .as_deref(),
            Some("off")
        );
        assert!(storage
            .delete_agent_memory(memories[0].id)
            .expect("memory should delete"));
        assert_eq!(
            storage
                .clear_agent_memories()
                .expect("remaining memories should clear"),
            1
        );
        assert!(storage
            .list_agent_memories()
            .expect("memories should be empty")
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_fast_mode_round_trips_and_defaults_off() {
        let path = test_db_path("conversation-fast-mode-round-trip");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Use fast mode", "gpt-5.6-sol", None)
            .expect("conversation should insert");

        assert!(!storage
            .conversation_fast_mode(&conversation_id)
            .expect("fast mode should load"));
        assert!(
            !storage
                .conversation_summary(&conversation_id)
                .expect("conversation should load")
                .fast_mode
        );

        assert!(
            storage
                .set_conversation_fast_mode(&conversation_id, true)
                .expect("fast mode should update")
                .fast_mode
        );
        assert!(storage
            .conversation_fast_mode(&conversation_id)
            .expect("fast mode should load"));

        drop(storage);
        let reopened = Storage::open(&path).expect("storage should reopen");
        assert!(
            reopened
                .conversation_summary(&conversation_id)
                .expect("conversation should load after reopen")
                .fast_mode
        );
        assert!(
            !reopened
                .set_conversation_fast_mode(&conversation_id, false)
                .expect("fast mode should turn off")
                .fast_mode
        );

        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_memory_cap_evicts_the_oldest_memory_in_each_scope() {
        let path = test_db_path("agent-memories-cap");
        let storage = Storage::open(&path).expect("storage should open");
        let contents = (0..=MAX_AGENT_MEMORIES_PER_SCOPE)
            .map(|index| format!("Durable memory {index:02}"))
            .collect::<Vec<_>>();

        storage
            .insert_agent_memories(None, &contents, None)
            .expect("memories should insert");
        let memories = storage
            .agent_memories_for_scope(None, 100)
            .expect("memories should list");
        assert_eq!(memories.len(), MAX_AGENT_MEMORIES_PER_SCOPE);
        assert!(!memories
            .iter()
            .any(|memory| memory.content == "Durable memory 00"));
        assert!(memories
            .iter()
            .any(|memory| memory.content == "Durable memory 50"));

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_memory_scope_never_returns_another_projects_memories() {
        let path = test_db_path("agent-memories-scope");
        let storage = Storage::open(&path).expect("storage should open");
        let root_a = test_dir_path("agent-memory-project-a");
        let root_b = test_dir_path("agent-memory-project-b");
        fs::create_dir_all(&root_a).expect("project A root should create");
        fs::create_dir_all(&root_b).expect("project B root should create");
        let project_a = storage
            .upsert_project("Project A", &root_a.to_string_lossy())
            .expect("project A should insert");
        let project_b = storage
            .upsert_project("Project B", &root_b.to_string_lossy())
            .expect("project B should insert");

        storage
            .insert_agent_memories(None, &["Global memory".to_string()], None)
            .expect("global memory should insert");
        storage
            .insert_agent_memories(Some(project_a.id), &["Project A memory".to_string()], None)
            .expect("project A memory should insert");
        storage
            .insert_agent_memories(Some(project_b.id), &["Project B memory".to_string()], None)
            .expect("project B memory should insert");

        let project_a_memories = storage
            .agent_memories_for_scope(Some(project_a.id), 30)
            .expect("project A scope should list");
        assert!(project_a_memories
            .iter()
            .any(|memory| memory.content == "Global memory"));
        assert!(project_a_memories
            .iter()
            .any(|memory| memory.content == "Project A memory"));
        assert!(!project_a_memories
            .iter()
            .any(|memory| memory.content == "Project B memory"));
        assert_eq!(
            storage
                .agent_memories_for_scope(None, 30)
                .expect("global scope should list")
                .into_iter()
                .map(|memory| memory.content)
                .collect::<Vec<_>>(),
            vec!["Global memory".to_string()]
        );

        drop(storage);
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir_all(root_a);
        let _ = fs::remove_dir_all(root_b);
    }

    #[test]
    fn message_attachments_round_trip_outside_message_text_and_cascade_on_delete() {
        let path = test_db_path("message-attachments");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Inspect this", "gpt-5.5", None)
            .expect("conversation should insert");
        let image_bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let inserted = storage
            .insert_message_with_attachments(
                NewMessage {
                    conversation_id: &conversation_id,
                    role: "user",
                    content: "Inspect this",
                    model: Some("gpt-5.5"),
                    input_tokens: None,
                    output_tokens: None,
                    error: false,
                    mentions: None,
                },
                &[NewMessageAttachment {
                    name: "screen.png".to_string(),
                    media_type: "image/png".to_string(),
                    data: image_bytes.clone(),
                }],
            )
            .expect("message and attachment should insert");

        assert_eq!(inserted.content, "Inspect this");
        assert_eq!(inserted.attachments.len(), 1);
        assert_eq!(inserted.attachments[0].name, "screen.png");
        assert_eq!(inserted.attachments[0].size_bytes, image_bytes.len() as u64);
        assert_eq!(inserted.attachments[0].data, STANDARD.encode(&image_bytes));

        let messages = storage
            .get_messages(&conversation_id)
            .expect("message attachments should load");
        assert_eq!(messages[0].attachments, inserted.attachments);
        assert!(!messages[0].content.contains("base64"));

        storage
            .delete_messages_from(&conversation_id, &inserted.id)
            .expect("message should delete");
        let connection = storage.connection.lock().expect("storage lock poisoned");
        let attachment_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM message_attachments", [], |row| {
                row.get(0)
            })
            .expect("attachment count should query");
        assert_eq!(attachment_count, 0);
        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn migration_is_idempotent_when_opening_db_twice() {
        let path = test_db_path("migration");
        let first = Storage::open(&path).expect("first open should migrate");
        drop(first);

        let second = Storage::open(&path).expect("second open should not rerun migration");
        let connection = second.connection.lock().expect("storage lock poisoned");
        let migration_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))
            .expect("migration count should query");
        assert_eq!(migration_count, 60);
        assert!(
            table_exists(&connection, "task_hierarchies").expect("hierarchy table should exist")
        );
        assert!(table_exists(&connection, "remote_devices").expect("remote devices should exist"));
        assert!(column_exists(&connection, "remote_devices", "host_id")
            .expect("remote device host id should inspect"));
        assert!(table_exists(&connection, "local_environment_approvals")
            .expect("local environment approvals table should inspect"));
        assert!(table_exists(&connection, "task_route_receipts").unwrap());
        assert!(table_exists(&connection, "task_route_agents").unwrap());
        assert!(table_exists(&connection, "task_routes").expect("task routes table should inspect"));
        assert!(table_exists(&connection, "session_checkpoints")
            .expect("session checkpoints table should inspect"));
        assert!(table_exists(&connection, "crux_index_state")
            .expect("Crux index state table should inspect"));
        assert!(
            column_exists(&connection, "savings_snapshots", "crux_tokens_saved")
                .expect("Crux savings column should inspect")
        );
        assert!(
            column_exists(&connection, "savings_snapshots", "occurred_local_date")
                .expect("snapshot local date column should inspect")
        );
        assert!(
            column_exists(&connection, "agent_session_turns", "occurred_local_date")
                .expect("turn local date column should inspect")
        );
        assert!(column_exists(
            &connection,
            "archived_savings_snapshots",
            "crux_tokens_saved"
        )
        .expect("archived Crux savings column should inspect"));
        assert!(column_exists(&connection, "conversations", "archived_at")
            .expect("archive column should inspect"));
        assert!(column_exists(&connection, "conversations", "memories_mode")
            .expect("memories mode column should inspect"));
        assert!(column_exists(&connection, "conversations", "mode")
            .expect("conversation mode column should inspect"));
        assert!(column_exists(&connection, "conversations", "pinned")
            .expect("conversation pinned column should inspect"));
        assert!(column_exists(&connection, "conversations", "draft")
            .expect("conversation draft column should inspect"));
        assert!(column_exists(&connection, "conversations", "draft_only")
            .expect("conversation draft-only column should inspect"));
        for table in ["agent_sessions", "agent_session_turns", "messages"] {
            assert!(column_exists(&connection, table, "ponytail_mode")
                .expect("Ponytail mode column should inspect"));
        }
        for column in [
            "default_title",
            "generated_title",
            "ai_vault_title",
            "ai_vault_session_id",
            "generation",
        ] {
            assert!(column_exists(&connection, "agent_sessions", column)
                .expect("agent title column should inspect"));
        }
        assert!(conversation_goal_column_exists(&connection).expect("goal column should inspect"));
        assert!(project_id_column_exists(&connection).expect("project column should inspect"));
        assert!(agent_mode_column_exists(&connection).expect("agent mode column should inspect"));
        assert!(project_trust_level_column_exists(&connection)
            .expect("project trust column should inspect"));
        assert!(project_approval_policy_column_exists(&connection)
            .expect("project approval policy column should inspect"));
        assert!(
            conversation_approval_policy_override_column_exists(&connection)
                .expect("conversation approval policy column should inspect")
        );
        assert!(
            messages_mentions_column_exists(&connection).expect("mentions column should inspect")
        );
        assert!(auth_session_table_exists(&connection).expect("auth table should inspect"));
        assert!(conversation_title_source_column_exists(&connection)
            .expect("title source column should inspect"));
        assert!(workspace_id_column_exists(&connection).expect("workspace column should inspect"));
        assert!(associated_worktree_id_column_exists(&connection)
            .expect("associated worktree column should inspect"));
        assert!(
            conversation_title_generation_attempted_column_exists(&connection)
                .expect("title generation column should inspect")
        );
        assert!(message_fts_table_exists(&connection).expect("FTS table should inspect"));
        assert!(table_exists(&connection, "archived_savings_snapshots")
            .expect("archived savings table should inspect"));
        assert!(table_exists(&connection, "message_attachments")
            .expect("message attachments table should inspect"));
        assert!(conversation_workflow_mode_column_exists(&connection)
            .expect("workflow mode column should inspect"));
        assert!(table_exists(&connection, "conversation_task_plans")
            .expect("task plans table should inspect"));
        assert!(table_exists(&connection, "subagent_threads")
            .expect("subagent threads table should inspect"));
        assert!(table_exists(&connection, "subagent_messages")
            .expect("subagent messages table should inspect"));
        assert!(table_exists(&connection, "conversation_compactions")
            .expect("conversation compactions table should inspect"));
        assert!(table_exists(&connection, "agent_memories")
            .expect("agent memories table should inspect"));
        assert!(table_exists(&connection, "savings_report_queue")
            .expect("savings report queue should inspect"));
        assert!(
            column_exists(&connection, "local_diagnostics", "free_turns_used")
                .expect("free-turn column should inspect")
        );
        assert!(
            column_exists(&connection, "local_diagnostics", "first_launch_at")
                .expect("first-launch column should inspect")
        );
        assert!(column_exists(&connection, "turn_snapshots", "after_exists")
            .expect("snapshot existence column should inspect"));
        assert!(column_exists(&connection, "turn_snapshots", "after_sha256")
            .expect("snapshot digest column should inspect"));
        let project_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'projects'",
                [],
                |row| row.get(0),
            )
            .expect("project table should query");
        assert_eq!(project_table_count, 1);

        drop(connection);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn crux_index_state_migration_persists_project_cache_data() {
        let path = test_db_path("crux-index-state");
        let storage = Storage::open(&path).expect("storage should open");
        let state = CruxIndexState {
            project_path: "/tmp/project".to_string(),
            last_indexed_git_head: Some("abc123".to_string()),
            last_index_stats: r#"{"files":200}"#.to_string(),
            last_index_time: "2026-08-12T12:00:00Z".to_string(),
        };
        storage
            .upsert_crux_index_state(&state)
            .expect("Crux index state should save");
        drop(storage);
        let reopened = Storage::open(&path).expect("storage should reopen");
        assert_eq!(
            reopened
                .crux_index_state(&state.project_path)
                .expect("Crux index state should load"),
            Some(state)
        );
        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn managed_workspace_binding_changes_the_conversation_runtime_root() {
        let path = test_db_path("managed-workspace-binding");
        let storage = Storage::open(&path).expect("storage should open");
        let project = storage
            .upsert_project("repo", "/tmp/halv-primary-workspace")
            .expect("project should insert");
        let primary = storage
            .list_workspaces(project.id)
            .expect("workspaces should list");
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].kind, "primary");

        let managed = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: project.id,
                name: "halv/worktree-test",
                root_path: "/tmp/halv-managed-workspace",
                git_common_dir: Some("/tmp/halv-primary-workspace/.git"),
                base_ref: Some("HEAD"),
                base_commit: Some("0123456789abcdef"),
                branch: Some("halv/worktree-test"),
                local_environment_config_path: Some(
                    "/tmp/halv-primary-workspace/.codex/environments/dev.toml",
                ),
            })
            .expect("managed workspace should insert");
        let conversation_id = storage
            .ensure_conversation_in_workspace(
                None,
                "Use the managed worktree",
                DEFAULT_STORAGE_MODEL,
                Some(project.id),
                Some(&managed.id),
            )
            .expect("conversation should bind workspace");
        let runtime_project = storage
            .project_for_conversation(&conversation_id)
            .expect("runtime project should load")
            .expect("runtime project should exist");
        assert_eq!(runtime_project.root_path, managed.root_path);
        assert_eq!(
            managed.local_environment_config_path.as_deref(),
            Some("/tmp/halv-primary-workspace/.codex/environments/dev.toml")
        );
        assert_eq!(
            storage
                .associated_worktree_for_conversation(&conversation_id)
                .expect("associated worktree should load")
                .expect("associated worktree should exist")
                .id,
            managed.id
        );
        let local = primary.first().expect("primary workspace should exist");
        storage
            .handoff_conversation_workspace(&conversation_id, &local.id, &managed.id)
            .expect("conversation should hand off to local");
        assert_eq!(
            storage
                .workspace_for_conversation(&conversation_id)
                .expect("local workspace should load")
                .expect("local workspace should exist")
                .id,
            local.id
        );
        assert_eq!(
            storage
                .associated_worktree_for_conversation(&conversation_id)
                .expect("associated worktree should survive local handoff")
                .expect("associated worktree should remain")
                .id,
            managed.id
        );
        storage
            .handoff_conversation_workspace(&conversation_id, &managed.id, &managed.id)
            .expect("conversation should hand back to worktree");
        assert_eq!(
            storage
                .workspace_for_conversation(&conversation_id)
                .expect("conversation workspace should load")
                .expect("conversation workspace should exist")
                .id,
            managed.id
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_summaries_name_their_workspace() {
        let path = test_db_path("conversation-summary-workspace");
        let storage = Storage::open(&path).expect("storage should open");
        let project = storage
            .upsert_project("repo", "/tmp/halv-summary-primary")
            .expect("project should insert");
        let primary_id = storage
            .list_workspaces(project.id)
            .expect("workspaces should list")[0]
            .id
            .clone();
        let worktree = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: project.id,
                name: "halv/summary",
                root_path: "/tmp/halv-summary-worktree",
                git_common_dir: Some("/tmp/halv-summary-primary/.git"),
                base_ref: Some("HEAD"),
                base_commit: Some("0123456789abcdef"),
                branch: Some("halv/summary"),
                local_environment_config_path: None,
            })
            .expect("managed workspace should insert");
        let on_primary = storage
            .set_conversation_goal(None, None, None, Some(project.id))
            .expect("primary conversation should insert");
        let on_worktree = storage
            .set_conversation_goal_in_workspace(
                None,
                None,
                None,
                Some(project.id),
                Some(&worktree.id),
            )
            .expect("worktree conversation should insert");
        let loose = storage
            .set_conversation_goal(None, None, None, None)
            .expect("loose conversation should insert");

        assert_eq!(
            on_primary.workspace_id.as_deref(),
            Some(primary_id.as_str())
        );
        assert_eq!(
            on_worktree.workspace_id.as_deref(),
            Some(worktree.id.as_str())
        );
        assert_eq!(
            on_worktree.project_root_path.as_deref(),
            Some(worktree.root_path.as_str())
        );
        assert_eq!(loose.workspace_id, None);
        let listed = storage
            .list_conversations()
            .expect("conversations should list");
        let workspace_of = |id: &str| {
            listed
                .iter()
                .find(|summary| summary.id == id)
                .expect("conversation should list")
                .workspace_id
                .clone()
        };
        assert_eq!(workspace_of(&on_primary.id), Some(primary_id));
        assert_eq!(workspace_of(&on_worktree.id), Some(worktree.id));
        assert_eq!(workspace_of(&loose.id), None);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn snapshot_postcondition_migration_disables_legacy_reverts() {
        let connection = Connection::open_in_memory().expect("memory database should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migrations table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("initial schema should create");
        apply_turn_diffs_migration(&connection).expect("turn diff migration should apply");
        connection
            .execute(
                "INSERT INTO conversations (id, title, model, created_at, updated_at)
                 VALUES ('conversation', 'Legacy', 'gpt-5.5', 'now', 'now')",
                [],
            )
            .expect("conversation should insert");
        connection
            .execute(
                "INSERT INTO turn_diffs
                    (conversation_id, turn_id, diff, cwd, created_at, revertible)
                 VALUES ('conversation', 'turn', 'diff', '/tmp/project', 'now', 1)",
                [],
            )
            .expect("legacy diff should insert");
        connection
            .execute(
                "INSERT INTO turn_snapshots
                    (conversation_id, turn_id, display_path, absolute_path, before, created_at)
                 VALUES ('conversation', 'turn', 'file.txt', '/tmp/project/file.txt', X'01', 'now')",
                [],
            )
            .expect("legacy snapshot should insert");

        apply_turn_snapshot_postconditions_migration(&connection)
            .expect("postcondition migration should apply");
        apply_turn_snapshot_postconditions_migration(&connection)
            .expect("postcondition migration should be idempotent");

        let revertible: i64 = connection
            .query_row(
                "SELECT revertible FROM turn_diffs WHERE turn_id = 'turn'",
                [],
                |row| row.get(0),
            )
            .expect("legacy diff should query");
        let postcondition: (Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT after_exists, after_sha256 FROM turn_snapshots WHERE turn_id = 'turn'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("legacy snapshot should query");
        assert_eq!(revertible, 0);
        assert_eq!(postcondition, (None, None));
    }

    #[test]
    fn storage_open_repairs_legacy_turn_diffs_without_revertible_column() {
        let path = test_db_path("legacy-turn-diffs-revertible");
        {
            let connection = Connection::open(&path).expect("legacy database should open");
            connection
                .execute_batch(MIGRATIONS_SQL)
                .expect("migrations table should create");
            connection
                .execute_batch(INITIAL_SCHEMA_SQL)
                .expect("initial schema should create");
            connection
                .execute_batch(
                    "CREATE TABLE turn_snapshots (
                        conversation_id TEXT NOT NULL,
                        turn_id TEXT NOT NULL,
                        display_path TEXT NOT NULL,
                        absolute_path TEXT NOT NULL,
                        before BLOB NULL,
                        created_at TEXT NOT NULL,
                        PRIMARY KEY (conversation_id, turn_id, absolute_path),
                        FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                    );

                    CREATE TABLE turn_diffs (
                        conversation_id TEXT NOT NULL,
                        turn_id TEXT NOT NULL,
                        diff TEXT NOT NULL,
                        cwd TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        PRIMARY KEY (conversation_id, turn_id),
                        FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                    );",
                )
                .expect("legacy turn tables should create");
            connection
                .execute(
                    "INSERT INTO conversations (id, title, model, created_at, updated_at)
                     VALUES ('conversation', 'Legacy', 'gpt-5.5', 'now', 'now')",
                    [],
                )
                .expect("legacy conversation should insert");
            connection
                .execute(
                    "INSERT INTO turn_diffs (conversation_id, turn_id, diff, cwd, created_at)
                     VALUES ('conversation', 'turn', 'diff', '/tmp/project', 'now')",
                    [],
                )
                .expect("legacy turn diff should insert");

            for version in [
                MIGRATION_INITIAL,
                MIGRATION_CONVERSATION_GOAL,
                MIGRATION_PROJECTS,
                MIGRATION_AGENT_MODE_TRUST,
                MIGRATION_APPROVAL_POLICY,
                MIGRATION_TURN_DIFFS,
                MIGRATION_MESSAGE_MENTIONS,
                MIGRATION_APPROVAL_POLICY_SKIP_ALL,
                MIGRATION_AUTH_SESSION,
                MIGRATION_SEARCH_AND_TITLES,
            ] {
                connection
                    .execute(
                        "INSERT INTO migrations (version, applied_at) VALUES (?1, 'now')",
                        params![version],
                    )
                    .expect("legacy migration should record");
            }
        }

        let storage = Storage::open(&path).expect("legacy database should migrate and open");
        let connection = storage.connection.lock().expect("storage lock poisoned");
        assert!(column_exists(&connection, "turn_diffs", "revertible")
            .expect("revertible column should inspect"));
        let revertible: i64 = connection
            .query_row(
                "SELECT revertible FROM turn_diffs WHERE turn_id = 'turn'",
                [],
                |row| row.get(0),
            )
            .expect("legacy turn diff should query");
        assert_eq!(revertible, 0);

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_open_rebuilds_legacy_turn_snapshots_primary_key_and_deduplicates() {
        let path = test_db_path("legacy-turn-snapshots-primary-key");
        {
            let connection = Connection::open(&path).expect("legacy database should open");
            connection
                .execute_batch(MIGRATIONS_SQL)
                .expect("migrations table should create");
            connection
                .execute_batch(INITIAL_SCHEMA_SQL)
                .expect("initial schema should create");
            connection
                .execute_batch(
                    "CREATE TABLE turn_snapshots (
                        conversation_id TEXT NOT NULL,
                        turn_id TEXT NOT NULL,
                        display_path TEXT NOT NULL,
                        absolute_path TEXT NOT NULL,
                        before BLOB NULL,
                        created_at TEXT NOT NULL,
                        after_exists INTEGER NULL
                            CHECK (after_exists IS NULL OR after_exists IN (0, 1)),
                        after_sha256 TEXT NULL,
                        PRIMARY KEY (conversation_id, turn_id, display_path),
                        FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                    );

                    CREATE TABLE turn_diffs (
                        conversation_id TEXT NOT NULL,
                        turn_id TEXT NOT NULL,
                        diff TEXT NOT NULL,
                        cwd TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        revertible INTEGER NOT NULL DEFAULT 1 CHECK (revertible IN (0, 1)),
                        PRIMARY KEY (conversation_id, turn_id),
                        FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                    );

                    CREATE INDEX idx_turn_snapshots_conversation_turn
                        ON turn_snapshots(conversation_id, turn_id);
                    CREATE INDEX idx_turn_diffs_conversation_created
                        ON turn_diffs(conversation_id, created_at ASC, turn_id ASC);",
                )
                .expect("legacy turn tables should create");
            connection
                .execute(
                    "INSERT INTO conversations (id, title, model, created_at, updated_at)
                     VALUES ('conversation', 'Legacy', 'gpt-5.5', 'now', 'now')",
                    [],
                )
                .expect("legacy conversation should insert");

            let first_digest = "1".repeat(64);
            connection
                .execute(
                    "INSERT INTO turn_snapshots
                        (conversation_id, turn_id, display_path, absolute_path, before,
                         created_at, after_exists, after_sha256)
                     VALUES ('conversation', 'turn', 'src/first.rs', '/tmp/project/shared.rs',
                             X'01', 'first', 1, ?1)",
                    params![first_digest],
                )
                .expect("first legacy snapshot should insert");
            connection
                .execute(
                    "INSERT INTO turn_snapshots
                        (conversation_id, turn_id, display_path, absolute_path, before,
                         created_at, after_exists, after_sha256)
                     VALUES ('conversation', 'turn', 'src/latest.rs', '/tmp/project/shared.rs',
                             X'02', 'latest', 0, NULL)",
                    [],
                )
                .expect("latest legacy snapshot should insert");

            for version in [
                MIGRATION_INITIAL,
                MIGRATION_CONVERSATION_GOAL,
                MIGRATION_PROJECTS,
                MIGRATION_AGENT_MODE_TRUST,
                MIGRATION_APPROVAL_POLICY,
                MIGRATION_TURN_DIFFS,
                MIGRATION_MESSAGE_MENTIONS,
                MIGRATION_APPROVAL_POLICY_SKIP_ALL,
                MIGRATION_AUTH_SESSION,
                MIGRATION_SEARCH_AND_TITLES,
                MIGRATION_TURN_SNAPSHOT_POSTCONDITIONS,
            ] {
                connection
                    .execute(
                        "INSERT INTO migrations (version, applied_at) VALUES (?1, 'now')",
                        params![version],
                    )
                    .expect("legacy migration should record");
            }
        }

        let storage = Storage::open(&path).expect("legacy database should rebuild and open");
        let connection = storage.connection.lock().expect("storage lock poisoned");
        assert!(
            column_primary_key_position(&connection, "turn_snapshots", "absolute_path")
                .expect("absolute path primary key position should inspect")
                .expect("absolute path column should exist")
                > 0
        );
        assert_eq!(
            column_primary_key_position(&connection, "turn_snapshots", "display_path")
                .expect("display path primary key position should inspect"),
            Some(0)
        );
        let snapshot_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM turn_snapshots", [], |row| row.get(0))
            .expect("snapshot count should query");
        assert_eq!(snapshot_count, 1);
        let survivor: (String, Vec<u8>, Option<i64>, Option<String>, String) = connection
            .query_row(
                "SELECT display_path, before, after_exists, after_sha256, created_at
                 FROM turn_snapshots",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("surviving snapshot should query");
        assert_eq!(
            survivor,
            (
                "src/latest.rs".to_string(),
                vec![2],
                Some(0),
                None,
                "latest".to_string(),
            )
        );
        let snapshot_index_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_turn_snapshots_conversation_turn'",
                [],
                |row| row.get(0),
            )
            .expect("snapshot index should query");
        assert_eq!(snapshot_index_count, 1);
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key pragma should query");
        assert_eq!(foreign_keys, 1);
        drop(connection);

        let updated_digest = "b".repeat(64);
        storage
            .insert_turn_snapshot_with_postcondition(
                "conversation",
                "turn",
                "src/latest.rs",
                Path::new("/tmp/project/shared.rs"),
                Some(&b"replacement"[..]),
                Some(true),
                Some(&updated_digest),
            )
            .expect("production snapshot upsert should succeed");
        let connection = storage.connection.lock().expect("storage lock poisoned");
        let postcondition: (Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT after_exists, after_sha256 FROM turn_snapshots",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("updated snapshot should query");
        assert_eq!(postcondition, (Some(1), Some(updated_digest)));

        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_open_does_not_rebuild_current_turn_snapshots_table() {
        let path = test_db_path("current-turn-snapshots-primary-key");
        let storage = Storage::open(&path).expect("current database should open");
        let original_table: (String, i64) = storage
            .connection
            .lock()
            .expect("storage lock poisoned")
            .query_row(
                "SELECT sql, rootpage FROM sqlite_master
                 WHERE type = 'table' AND name = 'turn_snapshots'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("current snapshot table should query");
        drop(storage);

        let reopened = Storage::open(&path).expect("current database should reopen");
        let reopened_table: (String, i64) = reopened
            .connection
            .lock()
            .expect("storage lock poisoned")
            .query_row(
                "SELECT sql, rootpage FROM sqlite_master
                 WHERE type = 'table' AND name = 'turn_snapshots'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("reopened snapshot table should query");
        assert_eq!(reopened_table, original_table);

        drop(reopened);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn auth_session_storage_round_trips_and_clear_keeps_server_url() {
        let path = test_db_path("auth-session");
        let storage = Storage::open(&path).expect("storage should open");
        assert_eq!(
            storage
                .get_auth_session()
                .expect("empty auth session should query"),
            None
        );

        storage
            .save_auth_session(&AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: None,
                email: None,
                plan: None,
                entitlements_json: None,
                synced_at: None,
            })
            .expect("server url should save");
        assert_eq!(
            storage
                .get_auth_session()
                .expect("server url row should query")
                .expect("server url row should exist"),
            AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: None,
                email: None,
                plan: None,
                entitlements_json: None,
                synced_at: None,
            }
        );

        let session = AuthSession {
            server_url: "https://api.halv.test".to_string(),
            token: Some("token-123".to_string()),
            email: Some("user@example.com".to_string()),
            plan: Some("pro".to_string()),
            entitlements_json: Some(
                r#"{"beta":true,"billingRequired":true,"canUseApp":false,"plan":"pro","message":"Subscribe","checkoutUrl":"https://checkout.example"}"#
                    .to_string(),
            ),
            synced_at: Some("2026-07-08T12:00:00+00:00".to_string()),
        };
        storage
            .save_auth_session(&session)
            .expect("session should save");
        assert_eq!(
            storage
                .get_auth_session()
                .expect("saved auth session should query"),
            Some(session)
        );

        assert!(storage
            .clear_auth_session_if_current("https://api.halv.test", Some("token-123"))
            .expect("session should clear"));
        assert_eq!(
            storage
                .get_auth_session()
                .expect("cleared auth session should query")
                .expect("server url should remain"),
            AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: None,
                email: None,
                plan: None,
                entitlements_json: None,
                synced_at: None,
            }
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn saving_session_with_new_server_url_replaces_existing_session() {
        let path = test_db_path("auth-server-change");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .save_auth_session(&AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: Some("token-123".to_string()),
                email: Some("user@example.com".to_string()),
                plan: Some("pro".to_string()),
                entitlements_json: Some(r#"{"canUseApp":true}"#.to_string()),
                synced_at: Some("2026-07-08T12:00:00+00:00".to_string()),
            })
            .expect("session should save");

        let replacement = AuthSession {
            server_url: "https://new-api.halv.test".to_string(),
            token: Some("token-456".to_string()),
            email: Some("user@example.com".to_string()),
            plan: Some("pro".to_string()),
            entitlements_json: Some(r#"{"canUseApp":true}"#.to_string()),
            synced_at: Some("2026-07-09T12:00:00+00:00".to_string()),
        };
        storage
            .save_auth_session(&replacement)
            .expect("server change should save");

        assert_eq!(
            storage
                .get_auth_session()
                .expect("changed auth session should query"),
            Some(replacement)
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn conditional_auth_session_writes_reject_stale_credentials() {
        let path = test_db_path("auth-session-cas");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .save_auth_session(&AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: Some("old-token".to_string()),
                email: Some("old@example.com".to_string()),
                plan: Some("free".to_string()),
                entitlements_json: None,
                synced_at: None,
            })
            .expect("old session should save");

        assert!(storage
            .clear_auth_session_if_current("https://api.halv.test", Some("old-token"))
            .expect("matching clear should succeed"));
        assert!(!storage
            .update_auth_session_if_current(
                "https://api.halv.test",
                "old-token",
                "old@example.com",
                "pro",
                r#"{"canUseApp":true}"#,
                "2026-07-09T12:00:00+00:00",
            )
            .expect("stale update should be checked"));

        storage
            .save_auth_session(&AuthSession {
                server_url: "https://api.halv.test".to_string(),
                token: Some("new-token".to_string()),
                email: Some("new@example.com".to_string()),
                plan: Some("free".to_string()),
                entitlements_json: None,
                synced_at: None,
            })
            .expect("new session should save");
        assert!(!storage
            .clear_auth_session_if_current("https://api.halv.test", Some("old-token"))
            .expect("stale clear should be checked"));

        assert!(storage
            .update_auth_session_if_current(
                "https://api.halv.test",
                "new-token",
                "new@example.com",
                "pro",
                r#"{"canUseApp":true}"#,
                "2026-07-09T12:00:00+00:00",
            )
            .expect("matching update should succeed"));
        let updated = storage
            .get_auth_session()
            .expect("updated session should query")
            .expect("updated session should exist");
        assert_eq!(updated.server_url, "https://api.halv.test");
        assert_eq!(updated.token.as_deref(), Some("new-token"));
        assert_eq!(updated.plan.as_deref(), Some("pro"));

        assert!(storage
            .clear_auth_session_if_current("https://api.halv.test", Some("new-token"),)
            .expect("matching clear should succeed"));
        let cleared = storage
            .get_auth_session()
            .expect("cleared session should query")
            .expect("server row should remain");
        assert_eq!(cleared.server_url, "https://api.halv.test");
        assert_eq!(cleared.token, None);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn auth_session_migration_is_idempotent_and_enforces_single_row() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migrations table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("initial schema should create");
        connection
            .execute(
                "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_INITIAL, now_string()],
            )
            .expect("initial migration should record");

        apply_auth_session_migration(&connection).expect("auth migration should apply");
        apply_auth_session_migration(&connection).expect("auth migration should be idempotent");

        assert!(auth_session_table_exists(&connection).expect("auth table should inspect"));
        connection
            .execute(
                "INSERT INTO auth_session (id, server_url) VALUES (1, 'https://api.halv.test')",
                [],
            )
            .expect("id 1 should satisfy auth session check");
        assert!(connection
            .execute(
                "INSERT INTO auth_session (id, server_url) VALUES (2, 'https://api.halv.test')",
                [],
            )
            .is_err());
    }

    #[test]
    fn bundled_sqlite_has_fts5_available() {
        let connection = Connection::open_in_memory().expect("connection should open");
        assert!(fts5_available(&connection).expect("FTS5 probe should run"));
    }

    #[test]
    fn message_search_fts_backfills_and_syncs_insert_update_delete() {
        let path = test_db_path("fts-sync");
        {
            let connection = Connection::open(&path).expect("connection should open");
            connection
                .execute_batch(MIGRATIONS_SQL)
                .expect("migrations table should create");
            connection
                .execute_batch(INITIAL_SCHEMA_SQL)
                .expect("initial schema should create");
            connection
                .execute(
                    "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
                    params![MIGRATION_INITIAL, now_string()],
                )
                .expect("initial migration should record");
            connection
                .execute(
                    "INSERT INTO conversations (id, title, model, created_at, updated_at)
                     VALUES ('conversation-search', 'Searchable thread', 'gpt-5.5', ?1, ?1)",
                    params![now_string()],
                )
                .expect("conversation should insert before FTS migration");
            connection
                .execute(
                    "INSERT INTO messages
                        (id, conversation_id, role, content, created_at, model, input_tokens, output_tokens, error)
                     VALUES
                        ('message-search', 'conversation-search', 'user', 'alpha beta first draft', ?1, 'gpt-5.5', NULL, NULL, 0)",
                    params![now_string()],
                )
                .expect("message should insert before FTS migration");
        }

        let storage = Storage::open(&path).expect("storage should migrate");
        assert_eq!(
            storage
                .search_messages("alpha")
                .expect("backfilled search should run")
                .iter()
                .map(|result| result.message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["message-search"]
        );

        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE messages SET content = 'gamma delta revised draft'
                     WHERE id = 'message-search'",
                    [],
                )
                .expect("message update should trigger FTS update");
        }
        assert!(storage
            .search_messages("alpha")
            .expect("old term search should run")
            .is_empty());
        assert_eq!(
            storage
                .search_messages("gamma")
                .expect("updated term search should run")
                .len(),
            1
        );

        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute("DELETE FROM messages WHERE id = 'message-search'", [])
                .expect("message delete should trigger FTS delete");
        }
        assert!(storage
            .search_messages("gamma")
            .expect("deleted term search should run")
            .is_empty());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_query_escaping_handles_quotes_asterisks_and_parens() {
        let path = test_db_path("fts-escaping");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Search syntax", "gpt-5.5", None)
            .expect("conversation should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Quotes, asterisks, and parens should never crash search.",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("message should insert");

        for query in ["\"", "*", "(", ")", "(asterisks*)", "\"parens\""] {
            storage
                .search_messages(query)
                .unwrap_or_else(|error| panic!("query {query:?} should not error: {error}"));
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn search_results_include_ranked_shape_and_highlighted_snippet() {
        let path = test_db_path("fts-shape");
        let storage = Storage::open(&path).expect("storage should open");
        let first_conversation_id = storage
            .ensure_conversation(None, "Needle one", "gpt-5.5", None)
            .expect("first conversation should insert");
        let first_message = storage
            .insert_message(NewMessage {
                conversation_id: &first_conversation_id,
                role: "assistant",
                content: "needle appears in the first assistant response",
                model: Some("gpt-5.5"),
                input_tokens: Some(1),
                output_tokens: Some(2),
                error: false,
                mentions: None,
            })
            .expect("first message should insert");
        let second_conversation_id = storage
            .ensure_conversation(None, "Needle two", "gpt-5.5", None)
            .expect("second conversation should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &second_conversation_id,
                role: "user",
                content: "another needle result",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("second message should insert");

        let results = storage
            .search_messages("needle")
            .expect("search should run");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|result| {
            result.conversation_id == first_conversation_id
                && result.conversation_title == "Needle one"
                && result.message_id == first_message.id
                && result.role == "assistant"
                && result.snippet.contains(SEARCH_HIGHLIGHT_START)
                && result.snippet.contains(SEARCH_HIGHLIGHT_END)
        }));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn title_generation_claim_is_once_and_respects_manual_source() {
        let path = test_db_path("title-claim");
        let storage = Storage::open(&path).expect("storage should open");
        let generated_id = storage
            .ensure_conversation(None, "Generate a better title", "gpt-5.5", None)
            .expect("conversation should insert");

        assert!(storage
            .claim_title_generation(&generated_id)
            .expect("first title claim should run"));
        assert!(!storage
            .claim_title_generation(&generated_id)
            .expect("second title claim should run"));
        let summary = storage
            .set_generated_conversation_title(&generated_id, "Better Thread Title")
            .expect("generated title should save")
            .expect("generated title should update");
        assert_eq!(summary.title, "Better Thread Title");
        assert!(!storage
            .claim_title_generation(&generated_id)
            .expect("generated source should block further claims"));

        let manual_id = storage
            .ensure_conversation(None, "Manual title stays", "gpt-5.5", None)
            .expect("manual conversation should insert");
        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE conversations
                     SET title = 'Manual Name', title_source = 'manual'
                     WHERE id = ?1",
                    params![manual_id],
                )
                .expect("manual title source should update");
        }
        assert!(!storage
            .claim_title_generation(&manual_id)
            .expect("manual title claim should run"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn manual_conversation_title_is_persisted_and_blocks_generation() {
        let path = test_db_path("manual-title");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Original title", "gpt-5.5", None)
            .expect("conversation should insert");
        let original_updated_at = storage
            .conversation_summary(&conversation_id)
            .expect("conversation should load")
            .updated_at;

        let summary = storage
            .set_conversation_title(&conversation_id, "  Release   audit  ")
            .expect("manual title should save");
        assert_eq!(summary.title, "Release audit");
        assert_eq!(summary.title_source, "manual");
        assert_eq!(summary.updated_at, original_updated_at);
        assert!(!storage
            .claim_title_generation(&conversation_id)
            .expect("manual title should not generate"));
        assert!(storage
            .set_conversation_title(&conversation_id, "")
            .expect_err("empty title should fail")
            .to_string()
            .contains("1 to 120"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn pinned_conversations_sort_first_without_changing_activity() {
        let path = test_db_path("conversation-pinned");
        let storage = Storage::open(&path).expect("storage should open");
        let pinned_id = storage
            .ensure_conversation(None, "Pin this", "gpt-5.5", None)
            .expect("pinned conversation should insert");
        let recent_id = storage
            .ensure_conversation(None, "Recent", "gpt-5.5", None)
            .expect("recent conversation should insert");
        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE conversations SET updated_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
                    params![pinned_id],
                )
                .expect("old activity should set");
            connection
                .execute(
                    "UPDATE conversations SET updated_at = '2026-08-20T00:00:00Z' WHERE id = ?1",
                    params![recent_id],
                )
                .expect("recent activity should set");
        }

        let before = storage
            .conversation_summary(&pinned_id)
            .expect("conversation should load")
            .updated_at;
        let pinned = storage
            .set_conversation_pinned(&pinned_id, true)
            .expect("conversation should pin");
        assert!(pinned.pinned);
        assert_eq!(pinned.updated_at, before);
        assert_eq!(
            storage
                .list_conversations()
                .expect("conversations should list")[0]
                .id,
            pinned_id
        );

        let unpinned = storage
            .set_conversation_pinned(&pinned_id, false)
            .expect("conversation should unpin");
        assert!(!unpinned.pinned);
        assert_eq!(
            storage
                .list_conversations()
                .expect("conversations should list")[0]
                .id,
            recent_id
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn unsent_drafts_persist_clear_on_send_and_delete_when_abandoned() {
        let path = test_db_path("conversation-draft");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Investigate the sidebar race", "gpt-5.6-sol", None)
            .expect("draft conversation should insert");
        let draft = storage
            .set_conversation_draft(&conversation_id, "Investigate the sidebar race", true)
            .expect("draft should save")
            .expect("draft row should remain");
        assert_eq!(draft.draft, "Investigate the sidebar race");
        assert!(draft.draft_only);
        assert_eq!(draft.title, "Investigate the sidebar race");
        drop(storage);

        let storage = Storage::open(&path).expect("storage should reopen");
        let restored = storage
            .conversation_summary(&conversation_id)
            .expect("draft should restore");
        assert_eq!(restored.draft, "Investigate the sidebar race");
        assert!(restored.draft_only);
        storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Investigate the sidebar race",
                model: Some("gpt-5.6-sol"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("message should insert");
        let sent = storage
            .conversation_summary(&conversation_id)
            .expect("sent conversation should load");
        assert!(sent.draft.is_empty());
        assert!(!sent.draft_only);

        let abandoned_id = storage
            .ensure_conversation(None, "Temporary draft", "gpt-5.6-sol", None)
            .expect("abandoned draft should insert");
        storage
            .set_conversation_draft(&abandoned_id, "Temporary draft", true)
            .expect("abandoned draft should save")
            .expect("abandoned draft should remain");
        assert!(storage
            .set_conversation_draft(&abandoned_id, "", false)
            .expect("empty draft should save")
            .is_none());
        assert!(storage.conversation_summary(&abandoned_id).is_err());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn touch_conversation_updates_activity_and_accepts_unknown_id() {
        let path = test_db_path("touch-conversation");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Activity", "gpt-5.5", None)
            .expect("conversation should insert");
        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE conversations SET updated_at = '2000-01-01T00:00:00Z' WHERE id = ?1",
                    params![conversation_id],
                )
                .expect("old activity should set");
        }

        storage
            .touch_conversation(&conversation_id)
            .expect("activity should update");
        let summary = storage
            .conversation_summary(&conversation_id)
            .expect("conversation should load");
        assert!(summary.updated_at.as_str() > "2000-01-01T00:00:00Z");
        storage
            .touch_conversation("missing-conversation")
            .expect("unknown conversation should be accepted");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_export_formats_markdown_and_json_with_code_blocks() {
        let path = test_db_path("export");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Export this thread", "gpt-5.5", None)
            .expect("conversation should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Show TypeScript",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("user message should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Here is code:\n\n```ts\nconst value = 1;\n```",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(8),
                error: false,
                mentions: None,
            })
            .expect("assistant message should insert");

        let markdown = storage
            .export_conversation(&conversation_id, ConversationExportFormat::Markdown)
            .expect("markdown export should render");
        assert!(markdown.starts_with("# Export this thread\n\n"));
        assert!(markdown.contains("## User\n\nShow TypeScript\n\n"));
        assert!(
            markdown.contains("## Assistant\n\nHere is code:\n\n```ts\nconst value = 1;\n```\n\n")
        );

        let json_text = storage
            .export_conversation(&conversation_id, ConversationExportFormat::Json)
            .expect("JSON export should render");
        let json: serde_json::Value =
            serde_json::from_str(&json_text).expect("JSON export should parse");
        assert_eq!(
            json.get("schema").and_then(serde_json::Value::as_str),
            Some("halv.conversation.export.v1")
        );
        assert_eq!(
            json.pointer("/conversation/title")
                .and_then(serde_json::Value::as_str),
            Some("Export this thread")
        );
        assert_eq!(
            json.pointer("/messages/1/content")
                .and_then(serde_json::Value::as_str),
            Some("Here is code:\n\n```ts\nconst value = 1;\n```")
        );
        assert_eq!(
            export_file_name(
                "  Export: this / thread?  ",
                &ConversationExportFormat::Markdown,
            ),
            "Export-this-thread.md"
        );
        assert_eq!(
            export_file_name("\u{0000}<>:*?", &ConversationExportFormat::Json),
            "conversation.json"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn project_add_list_remove_round_trip() {
        let path = test_db_path("project-round-trip");
        let storage = Storage::open(&path).expect("storage should open");
        let root = test_dir_path("project-root");
        fs::create_dir_all(&root).expect("project root should create");
        let root_path = root.to_string_lossy().to_string();

        let created = storage
            .upsert_project("Halv", &root_path)
            .expect("project should insert");
        assert_eq!(created.name, "Halv");
        assert_eq!(created.root_path, root_path);
        assert!(created.last_opened_at.is_some());

        let listed = storage.list_projects().expect("projects should list");
        assert_eq!(listed, vec![created.clone()]);

        let opened = storage
            .open_project(created.id)
            .expect("project should open");
        assert_eq!(opened.id, created.id);
        assert!(opened.last_opened_at.is_some());

        storage
            .remove_project(created.id)
            .expect("project should deregister");
        assert!(storage
            .list_projects()
            .expect("projects should list after remove")
            .is_empty());
        assert!(root.exists(), "project_remove must not delete files");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_project_binding_persists_and_nulls_on_project_remove() {
        let path = test_db_path("conversation-project");
        let storage = Storage::open(&path).expect("storage should open");
        let root = test_dir_path("bound-project");
        fs::create_dir_all(&root).expect("project root should create");
        let root_path = root.to_string_lossy().to_string();
        let project = storage
            .upsert_project("Bound", &root_path)
            .expect("project should insert");

        let conversation_id = storage
            .ensure_conversation(None, "Hello project", "gpt-5.5", Some(project.id))
            .expect("conversation should insert");
        let loaded = storage
            .project_for_conversation(&conversation_id)
            .expect("project should load")
            .expect("project should be bound");
        assert_eq!(loaded.id, project.id);

        storage
            .ensure_conversation(
                Some(conversation_id.clone()),
                "Second turn",
                "gpt-5.5",
                None,
            )
            .expect("existing conversation should remain bound");
        assert_eq!(
            storage
                .project_for_conversation(&conversation_id)
                .expect("project should load")
                .map(|project| project.id),
            Some(project.id)
        );

        let summaries = storage
            .list_conversations()
            .expect("conversations should list");
        assert_eq!(summaries[0].project_id, Some(project.id));
        assert_eq!(summaries[0].project_name.as_deref(), Some("Bound"));
        assert_eq!(
            summaries[0].project_root_path.as_deref(),
            Some(root_path.as_str())
        );
        assert_eq!(summaries[0].project_trust_level, None);
        assert!(!summaries[0].agent_mode);

        storage
            .set_project_trust(project.id, "trusted")
            .expect("project trust should set");
        assert!(
            storage
                .set_conversation_agent_mode(&conversation_id, true)
                .expect("agent mode should enable")
                .agent_mode
        );

        storage
            .remove_project(project.id)
            .expect("project should deregister");
        assert!(storage
            .project_for_conversation(&conversation_id)
            .expect("removed project should query")
            .is_none());
        assert_eq!(
            storage
                .list_conversations()
                .expect("conversations should list")[0]
                .project_id,
            None
        );
        assert!(
            !storage
                .conversation_agent_mode(&conversation_id)
                .expect("agent mode should query"),
            "detaching a project must clear agent mode before project_id is nulled"
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn agent_mode_requires_project_but_unset_trust_stays_eligible() {
        let path = test_db_path("agent-mode-trust");
        let storage = Storage::open(&path).expect("storage should open");

        let no_project_conversation = storage
            .ensure_conversation(None, "No project", "gpt-5.5", None)
            .expect("conversation should insert");
        let no_project_error = storage
            .set_conversation_agent_mode(&no_project_conversation, true)
            .expect_err("agent mode needs a project");
        assert!(no_project_error
            .to_string()
            .contains("Agent mode requires a project-bound conversation"));

        let root = test_dir_path("trusted-project");
        fs::create_dir_all(&root).expect("project root should create");
        let project = storage
            .upsert_project("Trusted", &root.to_string_lossy())
            .expect("project should insert");
        let conversation_id = storage
            .ensure_conversation(None, "Project", "gpt-5.5", Some(project.id))
            .expect("conversation should insert");

        let unset_enabled = storage
            .set_conversation_agent_mode(&conversation_id, true)
            .expect("unset trust still permits read-only agent mode");
        assert!(unset_enabled.agent_mode);
        assert_eq!(unset_enabled.project_trust_level, None);

        let untrusted = storage
            .set_project_trust(project.id, "untrusted")
            .expect("untrusted is a recorded decision");
        assert_eq!(untrusted.trust_level.as_deref(), Some("untrusted"));

        let enabled = storage
            .set_conversation_agent_mode(&conversation_id, true)
            .expect("agent mode should enable after trust decision");
        assert!(enabled.agent_mode);
        assert_eq!(enabled.project_trust_level.as_deref(), Some("untrusted"));
        assert!(storage
            .conversation_agent_mode(&conversation_id)
            .expect("agent mode should query"));

        let project = storage
            .set_project_approval_policy(project.id, "ask")
            .expect("project policy should set");
        assert_eq!(project.approval_policy.as_deref(), Some("ask"));

        let summary = storage
            .set_conversation_approval_policy(&conversation_id, Some("on_request"))
            .expect("conversation policy override should set");
        assert_eq!(
            summary.approval_policy_override.as_deref(),
            Some("on_request")
        );
        assert_eq!(
            storage
                .conversation_approval_policy_override(&conversation_id)
                .expect("policy override should query")
                .as_deref(),
            Some("on_request")
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(path);
    }

    fn lifecycle_route(id: &str, session: &str) -> TaskRoute {
        let created_at = "2026-01-01T00:00:00Z";
        TaskRoute {
            id: id.to_string(),
            session_id: session.to_string(),
            conversation_id: Some("conversation-1".to_string()),
            title: "Fix the login button".to_string(),
            brief_chars: 42,
            complexity: "standard".to_string(),
            complexity_confidence: 0.78,
            needs_reasoning: 0.12,
            risky: 0.03,
            tier: "cheap".to_string(),
            lane: "claude".to_string(),
            model: Some("sonnet".to_string()),
            effort: Some("high".to_string()),
            reasons: vec!["difficulty".to_string()],
            worker_lane: "auto".to_string(),
            status: "routed".to_string(),
            escalated_from_id: None,
            provider: "jev".to_string(),
            provider_model: "typesafe/jev-1.13".to_string(),
            latency_ms: 143,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            default_model: None,
            default_effort: None,
            default_source: None,
            closed_unreported: false,
            workflow: None,
            receipt: None,
        }
    }
    fn lifecycle_session<'a>(id: &'a str, status: &'a str) -> AgentSessionRecord<'a> {
        AgentSessionRecord {
            id,
            agent_kind: "codex",
            lane: "codex_chatgpt",
            mode: "terminal_embedded",
            ponytail_mode: None,
            project_id: None,
            conversation_id: None,
            cwd: "/tmp/project",
            cli_session_id: Some("thread-123"),
            custom_title: None,
            default_title: "Claude 1",
            generated_title: None,
            ai_vault_title: None,
            ai_vault_session_id: None,
            generation: 0,
            status,
            created_at: "2026-08-17T12:00:00Z",
            last_active_at: "2026-08-17T12:01:00Z",
            exit_code: None,
        }
    }

    #[test]
    fn session_exit_closes_routes_but_waiting_input_preserves_them() {
        let storage = Storage::open(test_db_path("route-exit")).unwrap();
        for id in ["owner", "other"] {
            storage
                .upsert_agent_session(&lifecycle_session(id, "running"))
                .unwrap();
            storage.insert_task_route(&lifecycle_route(id, id)).unwrap();
        }
        let mut completed = lifecycle_route("completed", "owner");
        completed.status = "passed".into();
        storage.insert_task_route(&completed).unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "waiting_input"))
            .unwrap();
        assert_eq!(
            storage.list_task_routes_for_session("owner").unwrap()[1].status,
            "routed"
        );
        storage
            .upsert_agent_session(&lifecycle_session("owner", "exited"))
            .unwrap();
        let routes = storage.list_task_routes_for_session("owner").unwrap();
        assert_eq!(routes[0], completed);
        assert_eq!(routes[1].status, "failed");
        assert_eq!(routes[1].reasons.last().unwrap(), SESSION_ENDED_REASON);
        storage
            .upsert_agent_session(&lifecycle_session("owner", "exited"))
            .unwrap();
        assert_eq!(
            storage.list_task_routes_for_session("owner").unwrap(),
            routes
        );
        assert_eq!(
            storage.list_task_routes_for_session("other").unwrap()[0].status,
            "routed"
        );
    }

    #[test]
    fn restart_reconciles_orphans_and_preserves_supervised_sessions() {
        let storage = Storage::open(test_db_path("route-restart")).unwrap();
        let mut live = std::collections::HashSet::new();
        for status in ["starting", "running", "waiting_input", "exited", "failed"] {
            storage
                .upsert_agent_session(&lifecycle_session(status, status))
                .unwrap();
            // Legacy versions could leave a route open after session exit.
            storage
                .insert_task_route(&lifecycle_route(status, status))
                .unwrap();
            if matches!(status, "starting" | "running" | "waiting_input") {
                let id = format!("live-{status}");
                storage
                    .upsert_agent_session(&lifecycle_session(&id, status))
                    .unwrap();
                storage
                    .insert_task_route(&lifecycle_route(&id, &id))
                    .unwrap();
                live.insert(id);
            }
        }
        storage.reconcile_session_routes(&live).unwrap();
        for status in ["starting", "running", "waiting_input", "exited", "failed"] {
            assert_eq!(
                storage.list_task_routes_for_session(status).unwrap()[0].status,
                "failed"
            );
        }
        for id in live {
            assert_eq!(
                storage.list_task_routes_for_session(&id).unwrap()[0].status,
                "routed"
            );
            assert_ne!(
                storage.get_agent_session(&id).unwrap().unwrap().status,
                "exited"
            );
        }
    }

    #[test]
    fn invalid_hierarchy_rolls_back_session_exit_and_route_changes() {
        let storage = Storage::open(test_db_path("route-rollback")).unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "running"))
            .unwrap();
        storage
            .insert_task_route(&lifecycle_route("route", "owner"))
            .unwrap();
        storage
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO task_hierarchies VALUES ('owner', 'invalid json', 'old')",
                [],
            )
            .unwrap();
        assert!(storage
            .upsert_agent_session(&lifecycle_session("owner", "exited"))
            .is_err());
        assert_eq!(
            storage.get_agent_session("owner").unwrap().unwrap().status,
            "running"
        );
        assert_eq!(
            storage.list_task_routes_for_session("owner").unwrap()[0].status,
            "routed"
        );
    }

    #[test]
    fn session_exit_updates_persisted_hierarchy_and_route_together() {
        let storage = Storage::open(test_db_path("route-graph")).unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "running"))
            .unwrap();
        storage
            .insert_task_route(&lifecycle_route("inv", "owner"))
            .unwrap();
        let mut graph = crate::task_hierarchy::Graph::default();
        let prepared = graph
            .prepare(&serde_json::json!({"stage":"investigation", "brief":"Inspect session exit"}))
            .unwrap();
        graph
            .accept(
                &prepared,
                serde_json::json!({
                    "taskId":"inv", "tier":"cheapest", "lane":"codex",
                    "model":"gpt-6.2-luna", "effort":"medium"
                }),
            )
            .unwrap();
        storage
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO task_hierarchies VALUES ('owner', ?1, 'old')",
                params![serde_json::to_string(&graph).unwrap()],
            )
            .unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "failed"))
            .unwrap();
        let json: String = storage
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT graph_json FROM task_hierarchies WHERE session_id='owner'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let graph: crate::task_hierarchy::Graph = serde_json::from_str(&json).unwrap();
        graph.validate().unwrap();
        assert!(!graph.has_active_root());
        assert_eq!(graph.routes["inv"].status, "failed");
        assert_eq!(
            storage.list_task_routes_for_session("owner").unwrap()[0].status,
            "failed"
        );
    }

    #[test]
    fn task_routes_persist_and_list_in_order() {
        let path = test_db_path("task-routes");
        let storage = Storage::open(&path).expect("storage should open");
        let route = |id: &str, created_at: &str| TaskRoute {
            id: id.to_string(),
            session_id: "session-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            title: "Fix the login button".to_string(),
            brief_chars: 42,
            complexity: "standard".to_string(),
            complexity_confidence: 0.78,
            needs_reasoning: 0.12,
            risky: 0.03,
            tier: "cheap".to_string(),
            lane: "claude".to_string(),
            model: Some("sonnet".to_string()),
            effort: Some("high".to_string()),
            reasons: vec!["difficulty".to_string()],
            worker_lane: "auto".to_string(),
            status: "routed".to_string(),
            escalated_from_id: None,
            provider: "jev".to_string(),
            provider_model: "typesafe/jev-1.13".to_string(),
            latency_ms: 143,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            default_model: None,
            default_effort: None,
            default_source: None,
            closed_unreported: false,
            workflow: None,
            receipt: None,
        };
        storage
            .insert_task_route(&route("route-2", "2026-09-22T10:01:00Z"))
            .expect("the second route should insert");
        storage
            .insert_task_route(&route("route-1", "2026-09-22T10:00:00Z"))
            .expect("the first route should insert");
        storage
            .update_task_route_status("route-1", "passed")
            .expect("the status should update");

        let routes = storage
            .list_task_routes_for_conversation("conversation-1")
            .expect("routes should list");
        assert_eq!(
            routes.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["route-1", "route-2"]
        );
        assert_eq!(routes[0].status, "passed");
        assert_eq!(routes[0].reasons, vec!["difficulty".to_string()]);
        assert_eq!(routes[0].effort.as_deref(), Some("high"));
        assert_eq!(routes[0].worker_lane, "auto");
        assert_eq!(
            storage
                .list_task_routes_for_session("session-1")
                .expect("session routes should list")
                .len(),
            2
        );
        assert!(storage
            .update_task_route_status("missing", "passed")
            .is_err());
    }

    #[test]
    fn task_route_receipts_migration_extends_an_old_route_table() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE migrations (version TEXT PRIMARY KEY, applied_at TEXT NOT NULL);",
            )
            .unwrap();
        connection.execute_batch(TASK_ROUTES_SQL).unwrap();
        let mut old = lifecycle_route("old-route", "session-1");
        old.workflow = None;
        connection
            .execute(
                "INSERT INTO task_routes (
                    id, session_id, title, brief_chars, complexity,
                    complexity_confidence, needs_reasoning, risky, tier, lane,
                    reasons_json, status, provider, provider_model, latency_ms,
                    created_at, updated_at
                 ) VALUES (?1, ?2, 'Old', 3, 'standard', 0.8, 0.1, 0.0, 'cheap',
                    'claude', '[]', 'passed', 'jev', 'jev-1', 1, ?3, ?3)",
                params![old.id, old.session_id, old.created_at],
            )
            .unwrap();
        assert!(!task_route_workflow_columns_exist(&connection).unwrap());

        apply_task_route_receipts_migration(&connection).unwrap();
        apply_task_route_receipts_migration(&connection).unwrap();

        assert!(task_route_workflow_columns_exist(&connection).unwrap());
        let routes = list_task_routes_rows(&connection, "session_id = ?1", "session-1").unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].workflow, None);
        let instances: i64 = connection
            .query_row(
                "SELECT instances FROM task_routes WHERE id = 'old-route'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(instances, 1);
        for table in ["task_route_receipts", "task_route_agents"] {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
    }

    fn workflow_route(id: &str, session: &str) -> TaskRoute {
        TaskRoute {
            default_model: Some("claude-opus-5-5".to_string()),
            default_effort: Some("xhigh".to_string()),
            default_source: Some("observed".to_string()),
            workflow: Some(TaskRouteWorkflow {
                batch_id: "batch-1".to_string(),
                name: Some("fix-findings".to_string()),
                key: "scout".to_string(),
                role: "read".to_string(),
                instances: 3,
            }),
            ..lifecycle_route(id, session)
        }
    }

    #[test]
    fn workflow_routes_round_trip_with_receipts_and_agents() {
        let storage = Storage::open(test_db_path("workflow-receipts")).unwrap();
        storage
            .insert_task_route(&workflow_route("scout", "session-1"))
            .unwrap();
        storage
            .insert_task_route(&lifecycle_route("legacy", "session-1"))
            .unwrap();
        let connection = storage.connection.lock().unwrap();
        let agent = |agent_id: &str, model: &str, output: i64| TaskRouteAgent {
            route_id: "scout".to_string(),
            agent_id: agent_id.to_string(),
            observed_model: model.to_string(),
            workflow_run_id: "wf_1".to_string(),
            label: "jev:scout:a".to_string(),
            observed_effort: Some("high".to_string()),
            input_tokens: 10,
            output_tokens: output,
            cache_read_tokens: 1_000,
            cache_write_5m_tokens: 150,
            cache_write_1h_tokens: 50,
            actual_usd: Some(0.01),
            price_version: Some("2026-10-15".to_string()),
            started_at: Some("2026-10-15T10:00:00Z".to_string()),
            ended_at: Some("2026-10-15T10:01:00Z".to_string()),
            observed_at: "2026-10-15T10:02:00Z".to_string(),
        };
        replace_task_route_agent_rows(
            &connection,
            "scout",
            &[
                agent("a1", "claude-sonnet-5-5", 30),
                agent("a2", "claude-sonnet-5-5", 40),
                agent("a2", "claude-opus-5-5", 5),
            ],
        )
        .unwrap();
        let receipt = TaskRouteReceipt {
            route_id: "scout".to_string(),
            session_id: "session-1".to_string(),
            outcome: "passed".to_string(),
            agents: 2,
            failed_agents: 0,
            input_tokens: Some(30),
            output_tokens: Some(75),
            cache_read_tokens: Some(3_000),
            cache_creation_tokens: Some(600),
            duration_ms: Some(60_000),
            usage_source: "measured".to_string(),
            reported_at: "2026-10-15T10:02:00Z".to_string(),
            models: Vec::new(),
        };
        upsert_task_route_receipt_row(&connection, &receipt).unwrap();
        upsert_task_route_receipt_row(
            &connection,
            &TaskRouteReceipt {
                outcome: "escalated".to_string(),
                ..receipt.clone()
            },
        )
        .unwrap();
        assert_eq!(
            list_task_route_agent_rows(&connection, "scout")
                .unwrap()
                .len(),
            3
        );
        drop(connection);

        let routes = storage.list_task_routes_for_session("session-1").unwrap();
        let scout = routes.iter().find(|route| route.id == "scout").unwrap();
        let workflow = scout.workflow.as_ref().unwrap();
        assert_eq!(workflow.key, "scout");
        assert_eq!(workflow.instances, 3);
        assert_eq!(scout.default_model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(scout.default_effort.as_deref(), Some("xhigh"));
        assert_eq!(scout.default_source.as_deref(), Some("observed"));
        let stored = scout.receipt.as_ref().unwrap();
        assert_eq!(stored.outcome, "escalated");
        assert_eq!(stored.output_tokens, Some(75));
        assert_eq!(
            stored.models,
            vec![
                "claude-opus-5-5".to_string(),
                "claude-sonnet-5-5".to_string()
            ]
        );
        let legacy = routes.iter().find(|route| route.id == "legacy").unwrap();
        assert_eq!(legacy.workflow, None);
        assert_eq!(legacy.receipt, None);
        let value = serde_json::to_value(scout).unwrap();
        assert_eq!(value["workflow"]["batchId"], "batch-1");
        assert_eq!(value["receipt"]["cacheCreationTokens"], 600);
    }

    #[test]
    fn session_exit_closes_unreported_workflow_routes() {
        let storage = Storage::open(test_db_path("workflow-session-exit")).unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "running"))
            .unwrap();
        storage
            .insert_task_route(&workflow_route("scout", "owner"))
            .unwrap();
        storage
            .upsert_agent_session(&lifecycle_session("owner", "exited"))
            .unwrap();
        let route = &storage.list_task_routes_for_session("owner").unwrap()[0];
        assert_eq!(route.status, "failed");
        assert!(route.closed_unreported);
        assert!(route.workflow.is_some());
    }

    #[test]
    fn workflow_mode_and_task_plan_persist_and_cascade_with_conversation() {
        let path = test_db_path("workflow-mode-plan");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Plan a refactor", "gpt-5.5", None)
            .expect("conversation should insert");

        assert_eq!(
            storage
                .conversation_workflow_mode(&conversation_id)
                .expect("workflow should load"),
            "default"
        );
        let summary = storage
            .set_conversation_workflow_mode(&conversation_id, "plan")
            .expect("plan mode should persist");
        assert_eq!(summary.workflow_mode, "plan");
        assert!(storage
            .set_conversation_workflow_mode(&conversation_id, "invalid")
            .is_err());

        let items = vec![
            TaskPlanItem {
                step: "Inspect".to_string(),
                status: "completed".to_string(),
            },
            TaskPlanItem {
                step: "Decide".to_string(),
                status: "in_progress".to_string(),
            },
        ];
        storage
            .upsert_conversation_task_plan(&conversation_id, Some("  Ready to plan  "), &items)
            .expect("task plan should persist");
        let loaded = storage
            .conversation_task_plan(&conversation_id)
            .expect("task plan should load")
            .expect("task plan should exist");
        assert_eq!(loaded.explanation.as_deref(), Some("Ready to plan"));
        assert_eq!(loaded.plan, items);

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");
        assert!(storage
            .conversation_task_plan(&conversation_id)
            .expect("task plan should query")
            .is_none());
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conversation_compaction_round_trip_returns_only_the_latest_row() {
        let path = test_db_path("conversation-compaction-round-trip");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Compact this thread", "gpt-5.5", None)
            .expect("conversation should insert");
        let first_message = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "First completed turn",
                model: Some("gpt-5.5"),
                input_tokens: Some(12),
                output_tokens: Some(4),
                error: false,
                mentions: None,
            })
            .expect("first message should insert");
        let first = storage
            .insert_conversation_compaction(
                &conversation_id,
                first_message.sequence,
                "First handoff summary",
            )
            .expect("first compaction should insert");
        assert_eq!(first.conversation_id, conversation_id);
        assert_eq!(first.upto_message_id, first_message.sequence);
        assert_eq!(first.summary, "First handoff summary");

        let second_message = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Second completed turn",
                model: Some("gpt-5.5"),
                input_tokens: Some(18),
                output_tokens: Some(6),
                error: false,
                mentions: None,
            })
            .expect("second message should insert");
        let second = storage
            .insert_conversation_compaction(
                &conversation_id,
                second_message.sequence,
                "Second handoff summary incorporating the first",
            )
            .expect("second compaction should insert");
        assert!(second.id > first.id);

        let latest = storage
            .latest_conversation_compaction(&conversation_id)
            .expect("latest compaction should query")
            .expect("latest compaction should exist");
        assert_eq!(latest, second);

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let stored_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_compactions WHERE conversation_id = ?1",
                params![conversation_id],
                |row| row.get(0),
            )
            .expect("compaction count should query");
        assert_eq!(stored_count, 2);
        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn forking_conversation_copies_message_history_and_settings_without_savings() {
        let path = test_db_path("conversation-fork-history-settings");
        let storage = Storage::open(&path).expect("storage should open");
        let project_root = test_dir_path("conversation-fork-project");
        let worktree_root = test_dir_path("conversation-fork-worktree");
        let project = storage
            .upsert_project("Fork project", &project_root.to_string_lossy())
            .expect("project should insert");
        let workspace = storage
            .insert_managed_workspace(NewWorkspace {
                project_id: project.id,
                name: "fork/settings-copy",
                root_path: &worktree_root.to_string_lossy(),
                git_common_dir: None,
                base_ref: Some("HEAD"),
                base_commit: Some("0123456789abcdef"),
                branch: Some("fork/settings-copy"),
                local_environment_config_path: None,
            })
            .expect("workspace should insert");
        let source_id = storage
            .ensure_conversation_in_workspace(
                None,
                "Original title",
                "gpt-5.6",
                Some(project.id),
                Some(&workspace.id),
            )
            .expect("source conversation should insert");
        storage
            .set_conversation_title(&source_id, "Original conversation")
            .expect("source title should set");
        storage
            .set_conversation_goal(
                Some(source_id.clone()),
                Some("Preserve this goal"),
                None,
                Some(project.id),
            )
            .expect("source goal should set");
        storage
            .set_conversation_agent_mode(&source_id, true)
            .expect("source agent mode should set");
        storage
            .set_conversation_workflow_mode(&source_id, "plan")
            .expect("source workflow mode should set");
        storage
            .set_conversation_approval_policy(&source_id, Some("on_failure"))
            .expect("source approval policy should set");
        storage
            .upsert_conversation_task_plan(
                &source_id,
                Some("Source-only plan"),
                &[TaskPlanItem {
                    step: "Do not copy this plan".to_string(),
                    status: "in_progress".to_string(),
                }],
            )
            .expect("source task plan should set");

        let mentions = vec!["src/main.rs".to_string()];
        let first = storage
            .insert_message_with_attachments(
                NewMessage {
                    conversation_id: &source_id,
                    role: "user",
                    content: "First user turn",
                    model: Some("gpt-5.6"),
                    input_tokens: Some(11),
                    output_tokens: None,
                    error: false,
                    mentions: Some(&mentions),
                },
                &[NewMessageAttachment {
                    name: "context.txt".to_string(),
                    media_type: "text/plain".to_string(),
                    data: b"fork context".to_vec(),
                }],
            )
            .expect("first message should insert");
        let second = storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "assistant",
                content: "First assistant reply",
                model: Some("gpt-5.6"),
                input_tokens: Some(17),
                output_tokens: Some(9),
                error: true,
                mentions: None,
            })
            .expect("second message should insert");
        storage
            .insert_savings_snapshot(&second.id, Some(123), Some(0.75))
            .expect("source savings should insert");
        let fork_point = storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "user",
                content: "Fork from this user turn",
                model: None,
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("fork point should insert");
        storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "assistant",
                content: "This reply must not be copied",
                model: Some("gpt-5.6"),
                input_tokens: Some(22),
                output_tokens: Some(13),
                error: false,
                mentions: None,
            })
            .expect("excluded message should insert");

        let source_prefix = storage
            .get_messages(&source_id)
            .expect("source messages should load")
            .into_iter()
            .take(3)
            .collect::<Vec<_>>();
        assert_eq!(source_prefix[0].id, first.id);
        let fork = storage
            .fork_conversation(&source_id, fork_point.sequence)
            .expect("conversation should fork");
        assert_eq!(fork.title, "Fork of Original conversation");
        assert_eq!(fork.model, "gpt-5.6");
        assert_eq!(fork.goal.as_deref(), Some("Preserve this goal"));
        assert!(fork.agent_mode);
        assert_eq!(fork.workflow_mode, "plan");
        assert_eq!(fork.project_id, Some(project.id));
        assert_eq!(fork.approval_policy_override.as_deref(), Some("on_failure"));
        assert_eq!(fork.message_count, 3);

        let forked_messages = storage
            .get_messages(&fork.id)
            .expect("forked messages should load");
        assert_eq!(forked_messages.len(), source_prefix.len());
        for (source, forked) in source_prefix.iter().zip(&forked_messages) {
            assert_ne!(forked.id, source.id);
            assert_ne!(forked.sequence, source.sequence);
            assert_eq!(forked.conversation_id, fork.id);
            assert_eq!(forked.role, source.role);
            assert_eq!(forked.content, source.content);
            assert_eq!(forked.created_at, source.created_at);
            assert_eq!(forked.model, source.model);
            assert_eq!(forked.input_tokens, source.input_tokens);
            assert_eq!(forked.output_tokens, source.output_tokens);
            assert_eq!(forked.error, source.error);
            assert_eq!(forked.mentions, source.mentions);
            assert_eq!(forked.attachments.len(), source.attachments.len());
            for (source_attachment, forked_attachment) in
                source.attachments.iter().zip(&forked.attachments)
            {
                assert_ne!(forked_attachment.id, source_attachment.id);
                assert_eq!(forked_attachment.name, source_attachment.name);
                assert_eq!(forked_attachment.media_type, source_attachment.media_type);
                assert_eq!(forked_attachment.size_bytes, source_attachment.size_bytes);
                assert_eq!(forked_attachment.data, source_attachment.data);
            }
            assert_eq!(forked.tokens_saved, None);
            assert_eq!(forked.savings_usd, None);
        }
        assert!(forked_messages
            .iter()
            .all(|message| message.content != "This reply must not be copied"));
        assert!(storage
            .conversation_task_plan(&fork.id)
            .expect("fork task plan should query")
            .is_none());

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let fork_settings: (Option<i64>, Option<String>, Option<String>, String, i64) = connection
            .query_row(
                "SELECT project_id, workspace_id, associated_worktree_id,
                        title_source, title_generation_attempted
                 FROM conversations
                 WHERE id = ?1",
                params![fork.id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("fork settings should query");
        assert_eq!(fork_settings.0, Some(project.id));
        assert_eq!(fork_settings.1.as_deref(), Some(workspace.id.as_str()));
        assert_eq!(fork_settings.2.as_deref(), Some(workspace.id.as_str()));
        assert_eq!(fork_settings.3, "manual");
        assert_eq!(fork_settings.4, 1);
        let fork_savings_count: i64 = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM savings_snapshots s
                 INNER JOIN messages m ON m.id = s.message_id
                 WHERE m.conversation_id = ?1",
                params![fork.id],
                |row| row.get(0),
            )
            .expect("fork savings count should query");
        assert_eq!(fork_savings_count, 0);
        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn forking_remaps_eligible_latest_compaction_and_supports_fork_of_a_fork() {
        let path = test_db_path("conversation-fork-compaction");
        let storage = Storage::open(&path).expect("storage should open");
        let source_id = storage
            .ensure_conversation(None, "Compaction source", "gpt-5.6", None)
            .expect("source conversation should insert");
        storage
            .set_conversation_title(&source_id, "Compaction source")
            .expect("source title should set");
        storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "user",
                content: "First turn",
                model: Some("gpt-5.6"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("first message should insert");
        let compacted_message = storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "assistant",
                content: "First reply",
                model: Some("gpt-5.6"),
                input_tokens: Some(12),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("compacted message should insert");
        let source_compaction = storage
            .insert_conversation_compaction(
                &source_id,
                compacted_message.sequence,
                "Compacted first turn",
            )
            .expect("source compaction should insert");
        let fork_point = storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "user",
                content: "Second turn",
                model: Some("gpt-5.6"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("fork point should insert");

        let first_fork = storage
            .fork_conversation(&source_id, fork_point.sequence)
            .expect("first fork should insert");
        let first_fork_messages = storage
            .get_messages(&first_fork.id)
            .expect("first fork messages should load");
        let first_fork_compaction = storage
            .latest_conversation_compaction(&first_fork.id)
            .expect("first fork compaction should query")
            .expect("eligible compaction should copy");
        assert_ne!(first_fork_compaction.id, source_compaction.id);
        assert_eq!(first_fork_compaction.summary, source_compaction.summary);
        assert_eq!(
            first_fork_compaction.created_at,
            source_compaction.created_at
        );
        assert_eq!(
            first_fork_compaction.upto_message_id,
            first_fork_messages[1].sequence
        );
        assert_ne!(
            first_fork_compaction.upto_message_id,
            source_compaction.upto_message_id
        );

        let second_fork = storage
            .fork_conversation(&first_fork.id, first_fork_messages[2].sequence)
            .expect("fork of a fork should insert");
        assert_eq!(second_fork.title, "Fork of Fork of Compaction source");
        let second_fork_messages = storage
            .get_messages(&second_fork.id)
            .expect("second fork messages should load");
        assert_eq!(
            second_fork_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["First turn", "First reply", "Second turn"]
        );
        let second_fork_compaction = storage
            .latest_conversation_compaction(&second_fork.id)
            .expect("second fork compaction should query")
            .expect("forked compaction should copy again");
        assert_eq!(
            second_fork_compaction.upto_message_id,
            second_fork_messages[1].sequence
        );
        assert_ne!(
            second_fork_compaction.upto_message_id,
            first_fork_compaction.upto_message_id
        );

        let later_message = storage
            .insert_message(NewMessage {
                conversation_id: &source_id,
                role: "assistant",
                content: "Later reply",
                model: Some("gpt-5.6"),
                input_tokens: Some(14),
                output_tokens: Some(6),
                error: false,
                mentions: None,
            })
            .expect("later message should insert");
        storage
            .insert_conversation_compaction(
                &source_id,
                later_message.sequence,
                "Latest compaction is beyond the fork point",
            )
            .expect("latest source compaction should insert");
        let fork_before_latest_compaction = storage
            .fork_conversation(&source_id, fork_point.sequence)
            .expect("fork before latest compaction should insert");
        assert!(storage
            .latest_conversation_compaction(&fork_before_latest_compaction.id)
            .expect("ineligible compaction should query")
            .is_none());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn deleting_conversation_removes_compaction_rows() {
        let path = test_db_path("conversation-compaction-delete");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Delete compacted thread", "gpt-5.5", None)
            .expect("conversation should insert");
        let message = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Completed turn",
                model: Some("gpt-5.5"),
                input_tokens: Some(12),
                output_tokens: Some(4),
                error: false,
                mentions: None,
            })
            .expect("message should insert");
        storage
            .insert_conversation_compaction(&conversation_id, message.sequence, "Summary to delete")
            .expect("compaction should insert");

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");

        assert!(storage
            .latest_conversation_compaction(&conversation_id)
            .expect("latest compaction should query")
            .is_none());
        let connection = storage.connection.lock().expect("storage lock poisoned");
        let stored_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM conversation_compactions", [], |row| {
                row.get(0)
            })
            .expect("compaction count should query");
        assert_eq!(stored_count, 0);
        drop(connection);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn subagent_threads_and_transcript_persist_recover_and_cascade() {
        let path = test_db_path("subagent-threads");
        let conversation_id;
        {
            let storage = Storage::open(&path).expect("storage should open");
            conversation_id = storage
                .ensure_conversation(None, "Delegate work", "gpt-5.5", None)
                .expect("conversation should insert");
            storage
                .upsert_subagent_thread(SubagentThreadUpdate {
                    agent_id: "agent-1",
                    conversation_id: &conversation_id,
                    turn_id: "turn-1",
                    task_name: "/root/tests",
                    task: "Run tests",
                    status: "running",
                    result: None,
                    error: None,
                    generation: 1,
                })
                .expect("thread should persist");
            storage
                .append_subagent_message("agent-1", "user", "Run tests", 1)
                .expect("task message should persist");
        }

        let storage = Storage::open(&path).expect("storage should reopen");
        let threads = storage
            .list_subagent_threads(&conversation_id)
            .expect("threads should list");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].status, "interrupted");
        assert_eq!(threads[0].messages.len(), 1);
        assert!(threads[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("restarted")));

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");
        assert!(storage
            .list_subagent_threads(&conversation_id)
            .expect("threads should query")
            .is_empty());
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn approval_policy_validation_accepts_new_policies() {
        validate_approval_policy("skip_all").expect("skip_all should validate");
        validate_approval_policy("on_failure").expect("on_failure should validate");
        assert!(validate_approval_policy("full_access").is_err());
    }

    #[test]
    fn approval_policy_migrations_preserve_values_and_accept_new_policies() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migrations table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("initial schema should create");
        connection
            .execute(
                "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_INITIAL, now_string()],
            )
            .expect("initial migration should record");
        apply_conversation_goal_migration(&connection).expect("goal migration should apply");
        apply_projects_migration(&connection).expect("projects migration should apply");
        apply_agent_mode_trust_migration(&connection).expect("trust migration should apply");
        apply_approval_policy_migration(&connection).expect("approval migration should apply");

        let created_at = now_string();
        connection
            .execute(
                "INSERT INTO projects
                    (id, name, root_path, created_at, last_opened_at, trust_level, approval_policy)
                 VALUES (1, 'Project', '/tmp/halv-project', ?1, NULL, 'trusted', 'never')",
                params![created_at],
            )
            .expect("project should insert");
        connection
            .execute(
                "INSERT INTO conversations
                    (id, title, model, created_at, updated_at, goal, project_id, agent_mode, approval_policy_override)
                 VALUES ('conversation-1', 'Conversation', 'gpt-5.5', ?1, ?1, NULL, 1, 1, 'ask')",
                params![created_at],
            )
            .expect("conversation should insert");

        apply_approval_policy_skip_all_migration(&connection)
            .expect("skip_all migration should apply");

        let project_policy: String = connection
            .query_row(
                "SELECT approval_policy FROM projects WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("project policy should query");
        assert_eq!(project_policy, "never");
        let conversation_policy: String = connection
            .query_row(
                "SELECT approval_policy_override FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .expect("conversation policy should query");
        assert_eq!(conversation_policy, "ask");

        connection
            .execute(
                "UPDATE projects SET approval_policy = 'skip_all' WHERE id = 1",
                [],
            )
            .expect("project skip_all should satisfy check");
        connection
            .execute(
                "UPDATE conversations
                 SET approval_policy_override = 'skip_all'
                 WHERE id = 'conversation-1'",
                [],
            )
            .expect("conversation skip_all should satisfy check");
        apply_approval_policy_skip_all_migration(&connection)
            .expect("skip_all migration should be idempotent");
        let skip_all_count: i64 = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM projects WHERE approval_policy = 'skip_all')
                    + (SELECT COUNT(*) FROM conversations WHERE approval_policy_override = 'skip_all')",
                [],
                |row| row.get(0),
            )
            .expect("skip_all values should query");
        assert_eq!(skip_all_count, 2);

        apply_approval_policy_on_failure_migration(&connection)
            .expect("on_failure migration should apply");
        connection
            .execute(
                "UPDATE projects SET approval_policy = 'on_failure' WHERE id = 1",
                [],
            )
            .expect("project on_failure should satisfy check");
        connection
            .execute(
                "UPDATE conversations
                 SET approval_policy_override = 'on_failure'
                 WHERE id = 'conversation-1'",
                [],
            )
            .expect("conversation on_failure should satisfy check");
        apply_approval_policy_on_failure_migration(&connection)
            .expect("on_failure migration should be idempotent");
        let on_failure_count: i64 = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM projects WHERE approval_policy = 'on_failure')
                    + (SELECT COUNT(*) FROM conversations WHERE approval_policy_override = 'on_failure')",
                [],
                |row| row.get(0),
            )
            .expect("on_failure values should query");
        assert_eq!(on_failure_count, 2);
    }

    #[test]
    fn clear_project_approval_policies_migration_runs_once() {
        let connection = Connection::open_in_memory().expect("connection should open");
        connection
            .execute_batch(MIGRATIONS_SQL)
            .expect("migrations table should create");
        connection
            .execute_batch(INITIAL_SCHEMA_SQL)
            .expect("initial schema should create");
        connection
            .execute(
                "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
                params![MIGRATION_INITIAL, now_string()],
            )
            .expect("initial migration should record");
        apply_conversation_goal_migration(&connection).expect("goal migration should apply");
        apply_projects_migration(&connection).expect("projects migration should apply");
        apply_agent_mode_trust_migration(&connection).expect("trust migration should apply");
        apply_approval_policy_migration(&connection).expect("approval migration should apply");
        apply_approval_policy_skip_all_migration(&connection)
            .expect("skip_all migration should apply");

        let created_at = now_string();
        for (id, policy) in [
            (1, "read_only"),
            (2, "ask"),
            (3, "on_request"),
            (4, "never"),
            (5, "skip_all"),
        ] {
            connection
                .execute(
                    "INSERT INTO projects
                        (id, name, root_path, created_at, last_opened_at, trust_level,
                         approval_policy)
                     VALUES (?1, ?2, ?3, ?4, NULL, 'trusted', ?5)",
                    params![
                        id,
                        format!("Project {id}"),
                        format!("/tmp/halv-project-{id}"),
                        created_at,
                        policy,
                    ],
                )
                .expect("project should insert");
        }
        connection
            .execute(
                "INSERT INTO conversations
                    (id, title, model, created_at, updated_at, project_id,
                     approval_policy_override)
                 VALUES ('conversation-1', 'Conversation', 'gpt-5.5', ?1, ?1, 1, 'ask')",
                params![created_at],
            )
            .expect("conversation should insert");

        apply_clear_project_approval_policies_migration(&connection)
            .expect("clear migration should apply");
        let project_policies = connection
            .prepare("SELECT approval_policy FROM projects ORDER BY id")
            .expect("project policy query should prepare")
            .query_map([], |row| row.get::<_, Option<String>>(0))
            .expect("project policies should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("project policies should collect");
        assert_eq!(
            project_policies,
            vec![
                Some("read_only".to_string()),
                Some("ask".to_string()),
                None,
                None,
                None,
            ]
        );
        let conversation_policy: Option<String> = connection
            .query_row(
                "SELECT approval_policy_override FROM conversations WHERE id = 'conversation-1'",
                [],
                |row| row.get(0),
            )
            .expect("conversation policy should query");
        assert_eq!(conversation_policy.as_deref(), Some("ask"));

        connection
            .execute(
                "UPDATE projects SET approval_policy = 'on_request' WHERE id = 3",
                [],
            )
            .expect("project policy should update after migration");
        apply_clear_project_approval_policies_migration(&connection)
            .expect("clear migration should be idempotent");
        let project_policy: Option<String> = connection
            .query_row(
                "SELECT approval_policy FROM projects WHERE id = 3",
                [],
                |row| row.get(0),
            )
            .expect("project policy should query again");
        assert_eq!(project_policy.as_deref(), Some("on_request"));
        let marker_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                params![MIGRATION_CLEAR_PROJECT_APPROVAL_POLICIES],
                |row| row.get(0),
            )
            .expect("migration marker should query");
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn storage_set_conversation_goal_creates_updates_and_clears_goal() {
        let path = test_db_path("conversation-goal");
        let storage = Storage::open(&path).expect("storage should open");

        let created = storage
            .set_conversation_goal(
                None,
                Some(" Keep responses focused on the release checklist "),
                Some("gpt-5.5"),
                None,
            )
            .expect("goal-only conversation should insert");
        assert_eq!(
            created.goal.as_deref(),
            Some("Keep responses focused on the release checklist")
        );
        assert_eq!(created.message_count, 0);
        assert!(created.title.starts_with("Goal: Keep responses focused"));
        assert_eq!(
            storage
                .conversation_goal(&created.id)
                .expect("goal should load")
                .as_deref(),
            Some("Keep responses focused on the release checklist")
        );

        let updated = storage
            .set_conversation_goal(
                Some(created.id.clone()),
                Some("Prefer concise answers"),
                Some("gpt-5.4"),
                None,
            )
            .expect("goal should update");
        assert_eq!(updated.goal.as_deref(), Some("Prefer concise answers"));
        assert_eq!(updated.model, "gpt-5.4");

        let cleared = storage
            .set_conversation_goal(Some(created.id.clone()), Some("   "), None, None)
            .expect("blank goal should clear");
        assert_eq!(cleared.goal, None);
        assert_eq!(
            storage
                .conversation_goal(&created.id)
                .expect("cleared goal should load"),
            None
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_total_savings_sums_app_snapshots_and_current_session() {
        let path = test_db_path("total-savings");
        {
            let storage = Storage::open(&path).expect("storage should open");
            let conversation_id = storage
                .ensure_conversation(None, "Old savings", "gpt-5.5", None)
                .expect("conversation should insert");
            let assistant = storage
                .insert_message(NewMessage {
                    conversation_id: &conversation_id,
                    role: "assistant",
                    content: "Old response",
                    model: Some("gpt-5.5"),
                    input_tokens: Some(20),
                    output_tokens: Some(10),
                    error: false,
                    mentions: None,
                })
                .expect("assistant message should insert");
            storage
                .insert_savings_snapshot(&assistant.id, Some(30), Some(0.12))
                .expect("old savings snapshot should insert");

            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2000-01-01T00:00:00+00:00", assistant.id],
                )
                .expect("old created_at should update");
        }

        let storage = Storage::open(&path).expect("storage should reopen");
        let conversation_id = storage
            .ensure_conversation(None, "New savings", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "New response",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("assistant message should insert");
        storage
            .insert_savings_snapshot(&assistant.id, Some(12), Some(0.04))
            .expect("new savings snapshot should insert");

        let totals = storage
            .total_savings()
            .expect("savings totals should query");
        assert_eq!(totals.tokens_saved, 42);
        assert!((totals.savings_usd - 0.16).abs() < 0.000_000_001);
        assert_eq!(totals.session_tokens_saved, 12);
        assert!((totals.session_savings_usd - 0.04).abs() < 0.000_000_001);
        assert_eq!(totals.crux_tokens_saved, 0);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_daily_token_activity_buckets_gui_messages_and_terminal_turns() {
        let path = test_db_path("daily-token-activity");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Daily activity", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "GUI response",
                model: Some("gpt-5.5"),
                input_tokens: Some(120),
                output_tokens: Some(30),
                error: false,
                mentions: None,
            })
            .expect("GUI message should insert");
        storage
            .insert_savings_snapshot(&assistant.id, Some(45), Some(0.01))
            .expect("GUI savings snapshot should insert");

        let gui_created_at = "2026-07-03T12:00:00+00:00";
        let terminal_created_at = "2026-07-05T12:00:00+00:00";
        let connection = storage.connection.lock().expect("storage lock poisoned");
        connection
            .execute(
                "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                params![gui_created_at, assistant.id],
            )
            .expect("GUI message timestamp should update");
        connection
            .execute(
                "UPDATE savings_snapshots
                 SET occurred_local_date = date(?1, 'localtime')
                 WHERE message_id = ?2",
                params![gui_created_at, assistant.id],
            )
            .expect("GUI snapshot local date should update");
        connection
            .execute(
                "INSERT INTO agent_sessions
                   (id, agent_kind, lane, mode, cwd, status, created_at, last_active_at)
                 VALUES ('daily-terminal', 'codex', 'codex_chatgpt', 'terminal_embedded',
                         '/tmp', 'completed', ?1, ?1)",
                params![terminal_created_at],
            )
            .expect("terminal session should insert");
        connection
            .execute(
                "INSERT INTO agent_session_turns
                   (session_id, turn_index, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, cost_usd,
                    savings_tokens, savings_source, created_at, occurred_local_date)
                 VALUES ('daily-terminal', 0, 80, 20, 0, 0, NULL, 25, 'measured',
                         ?1, date(?1, 'localtime'))",
                params![terminal_created_at],
            )
            .expect("terminal turn should insert");
        let gui_date = connection
            .query_row(
                "SELECT date(?1, 'localtime')",
                params![gui_created_at],
                |row| row.get::<_, String>(0),
            )
            .expect("GUI local date should resolve");
        let terminal_date = connection
            .query_row(
                "SELECT date(?1, 'localtime')",
                params![terminal_created_at],
                |row| row.get::<_, String>(0),
            )
            .expect("terminal local date should resolve");
        drop(connection);

        let activity = storage
            .daily_token_activity("2026-07-01T00:00:00+00:00")
            .expect("daily token activity should query");
        assert_eq!(
            activity,
            vec![
                DailyTokenActivity {
                    date: gui_date,
                    input_tokens: 120,
                    output_tokens: 30,
                    tokens_saved: 45,
                    dollars_usd: 0.0,
                },
                DailyTokenActivity {
                    date: terminal_date,
                    input_tokens: 80,
                    output_tokens: 20,
                    tokens_saved: 25,
                    dollars_usd: 0.0,
                },
            ]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_savings_today_sums_the_selected_local_date() {
        let path = test_db_path("today-savings");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Today savings", "gpt-5.5", None)
            .expect("conversation should insert");

        let old_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Old response",
                model: Some("gpt-5.5"),
                input_tokens: Some(20),
                output_tokens: Some(10),
                error: false,
                mentions: None,
            })
            .expect("old assistant message should insert");
        storage
            .insert_savings_snapshot(&old_assistant.id, Some(30), Some(0.12))
            .expect("old savings snapshot should insert");

        let user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "User messages do not count",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("user message should insert");
        storage
            .insert_savings_snapshot(&user.id, Some(99), Some(0.99))
            .expect("user savings snapshot should insert");

        let today_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Today response",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("today assistant message should insert");
        storage
            .insert_savings_snapshot(&today_assistant.id, Some(12), Some(0.04))
            .expect("today savings snapshot should insert");

        let null_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Null savings response",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("null assistant message should insert");
        storage
            .insert_savings_snapshot(&null_assistant.id, None, None)
            .expect("null savings snapshot should insert");

        let local_date = {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2000-01-01T00:00:00+00:00", old_assistant.id],
                )
                .expect("old assistant created_at should update");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-07-03T10:00:00+00:00", user.id],
                )
                .expect("user created_at should update");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-07-03T11:00:00+00:00", today_assistant.id],
                )
                .expect("today assistant created_at should update");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-07-03T12:00:00+00:00", null_assistant.id],
                )
                .expect("null assistant created_at should update");
            connection
                .execute(
                    "UPDATE savings_snapshots
                     SET occurred_local_date = (
                       SELECT date(m.created_at, 'localtime')
                       FROM messages m
                       WHERE m.id = savings_snapshots.message_id
                     )",
                    [],
                )
                .expect("snapshot local dates should update");
            connection
                .query_row(
                    "SELECT date('2026-07-03T11:00:00+00:00', 'localtime')",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("today local date should resolve")
        };

        let today = storage
            .savings_on_local_date(&local_date)
            .expect("today savings should query");
        assert_eq!(today.tokens_saved, 12);
        assert!((today.savings_usd - 0.04).abs() < 0.000_000_001);
        assert_eq!(today.recent_savings, vec![12]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_savings_since_compares_instants_across_offsets() {
        let path = test_db_path("today-offset-boundary");
        let storage = Storage::open(&path).expect("storage should open");
        let connection = storage.connection.lock().expect("storage lock poisoned");
        connection
            .execute(
                "INSERT INTO agent_sessions
                   (id, agent_kind, lane, mode, cwd, status, created_at, last_active_at)
                 VALUES ('offset-session', 'codex', 'codex_chatgpt', 'terminal_embedded',
                         '/tmp', 'completed', ?1, ?1)",
                params!["2026-07-02T21:00:00Z"],
            )
            .expect("session should insert");
        for (turn_index, tokens, cost_usd, created_at) in [
            (0, 1_500_000, None, "2026-07-02T21:59:59Z"),
            (1, 1_000_000, None, "2026-07-02T22:30:00Z"),
            (2, 1_000, Some(5.0), "2026-07-02T23:00:00Z"),
        ] {
            connection
                .execute(
                    "INSERT INTO agent_session_turns
                       (session_id, turn_index, input_tokens, output_tokens,
                        cache_read_tokens, cache_creation_tokens, cost_usd,
                        savings_tokens, savings_usd, savings_source, created_at)
                     VALUES ('offset-session', ?1, 0, 0, 0, 0, ?2, ?3, NULL,
                             'measured', ?4)",
                    params![turn_index, cost_usd, tokens, created_at],
                )
                .expect("turn should insert");
        }
        drop(connection);

        let totals = storage.total_savings().expect("total savings should query");
        assert_eq!(totals.tokens_saved, 2_501_000);
        assert!((totals.savings_usd - 3.126_25).abs() < 0.000_000_001);
        assert_ne!(totals.savings_usd, 5.0);

        let today = storage
            .savings_since("2026-07-03T00:00:00+02:00")
            .expect("offset savings should query");
        assert_eq!(today.tokens_saved, 1_001_000);
        assert!((today.savings_usd - 1.251_25).abs() < 0.000_000_001);
        assert_ne!(today.savings_usd, 5.0);
        assert_eq!(today.recent_savings, vec![1_000_000, 1_000]);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_savings_week_sums_trailing_seven_day_records() {
        let path = test_db_path("week-savings");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Week savings", "gpt-5.5", None)
            .expect("conversation should insert");
        let outside_week = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Outside trailing week",
                model: Some("gpt-5.5"),
                input_tokens: Some(20),
                output_tokens: Some(10),
                error: false,
                mentions: None,
            })
            .expect("old assistant message should insert");
        storage
            .insert_savings_snapshot(&outside_week.id, Some(20_300_000), Some(63.00))
            .expect("old savings snapshot should insert");
        let at_cutoff = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "At trailing week cutoff",
                model: Some("gpt-5.5"),
                input_tokens: Some(20),
                output_tokens: Some(10),
                error: false,
                mentions: None,
            })
            .expect("cutoff assistant message should insert");
        storage
            .insert_savings_snapshot(&at_cutoff.id, Some(4_000_000), Some(12.00))
            .expect("cutoff savings snapshot should insert");
        let inside_week = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Inside trailing week",
                model: Some("gpt-5.5"),
                input_tokens: Some(20),
                output_tokens: Some(10),
                error: false,
                mentions: None,
            })
            .expect("recent assistant message should insert");
        storage
            .insert_savings_snapshot(&inside_week.id, Some(6_000_000), Some(18.00))
            .expect("recent savings snapshot should insert");

        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-08-10T11:59:59+00:00", outside_week.id],
                )
                .expect("old assistant created_at should update");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-08-10T12:00:00+00:00", at_cutoff.id],
                )
                .expect("cutoff assistant created_at should update");
            connection
                .execute(
                    "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                    params!["2026-08-17T11:59:59+00:00", inside_week.id],
                )
                .expect("recent assistant created_at should update");
        }

        let now = chrono::DateTime::parse_from_rfc3339("2026-08-17T12:00:00+00:00")
            .expect("test time should parse")
            .with_timezone(&Utc);
        let cutoff = trailing_week_cutoff(now);
        assert_eq!(cutoff, "2026-08-10T12:00:00+00:00");

        let week = storage
            .savings_since(&cutoff)
            .expect("week savings should query");
        assert_eq!(week.tokens_saved, 10_000_000);
        assert!((week.savings_usd - 30.00).abs() < 0.000_000_001);
        assert_eq!(week.recent_savings, vec![4_000_000, 6_000_000]);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn turn_diff_storage_preserves_first_snapshot_and_appends_diff() {
        let path = test_db_path("turn-diff-storage");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Track a turn", "gpt-5.5", None)
            .expect("conversation should insert");
        let cwd = test_dir_path("turn-diff-cwd");
        fs::create_dir_all(&cwd).expect("cwd should create");
        let absolute_path = cwd.join("src/main.ts");
        let turn_id = "turn-1";

        storage
            .insert_turn_snapshot(
                &conversation_id,
                turn_id,
                "src/main.ts",
                &absolute_path,
                Some(b"before-one"),
            )
            .expect("first snapshot should insert");
        storage
            .insert_turn_snapshot(
                &conversation_id,
                turn_id,
                "./src/../src/main.ts",
                &absolute_path,
                Some(b"before-two"),
            )
            .expect("second snapshot should be ignored");
        let first_postimage = "a".repeat(64);
        let final_postimage = "b".repeat(64);
        storage
            .insert_turn_snapshot_with_postcondition(
                &conversation_id,
                turn_id,
                "src/main.ts",
                &absolute_path,
                Some(b"before-two"),
                Some(true),
                Some(&first_postimage),
            )
            .expect("first postcondition should update");
        storage
            .insert_turn_snapshot_with_postcondition(
                &conversation_id,
                turn_id,
                "src/main.ts",
                &absolute_path,
                Some(b"before-three"),
                Some(true),
                Some(&final_postimage),
            )
            .expect("latest postcondition should update");
        storage
            .upsert_turn_diff(&conversation_id, turn_id, &cwd, "diff-a\n", true)
            .expect("first diff should insert");
        storage
            .upsert_turn_diff(&conversation_id, turn_id, &cwd, "diff-b\n", true)
            .expect("second diff should append");

        let snapshots = storage
            .get_turn_snapshots(&conversation_id, turn_id)
            .expect("snapshots should list");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].display_path, "src/main.ts");
        assert_eq!(snapshots[0].absolute_path, absolute_path);
        assert_eq!(snapshots[0].before.as_deref(), Some(&b"before-one"[..]));
        assert_eq!(snapshots[0].after_exists, Some(true));
        assert_eq!(
            snapshots[0].after_sha256.as_deref(),
            Some(final_postimage.as_str())
        );

        let diffs = storage
            .list_turn_diffs(&conversation_id)
            .expect("diffs should list");
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].turn_id, turn_id);
        assert_eq!(diffs[0].diff, "diff-a\ndiff-b\n");
        assert_eq!(diffs[0].cwd, cwd.to_string_lossy());
        assert!(diffs[0].revertible);

        storage
            .delete_turn(&conversation_id, turn_id)
            .expect("turn should delete");
        assert!(storage
            .list_turn_diffs(&conversation_id)
            .expect("diffs should list after delete")
            .is_empty());
        assert!(storage
            .get_turn_snapshots(&conversation_id, turn_id)
            .expect("snapshots should list after delete")
            .is_empty());

        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn delete_messages_from_returns_typed_error_for_missing_anchor() {
        let path = test_db_path("delete-messages-missing-anchor");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Keep the existing turn", "gpt-5.5", None)
            .expect("conversation should insert");
        let existing_message = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Existing",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("message should insert");
        let stale_message_id = "stale-message-id";

        let error = storage
            .delete_messages_from(&conversation_id, stale_message_id)
            .expect_err("missing anchor should fail");

        match error {
            StorageError::MessageNotFound {
                conversation_id: missing_conversation_id,
                message_id: missing_message_id,
            } => {
                assert_eq!(missing_conversation_id, conversation_id);
                assert_eq!(missing_message_id, stale_message_id);
            }
            other => panic!("expected typed message-not-found error, got {other}"),
        }
        let messages = storage
            .get_messages(&conversation_id)
            .expect("existing messages should remain");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, existing_message.id);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn delete_messages_from_removes_anchor_later_messages_and_dependents() {
        let path = test_db_path("delete-messages-from");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Keep the first turn", "gpt-5.5", None)
            .expect("conversation should insert");
        let first_user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "First",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("first user should insert");
        let first_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "First reply",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("first assistant should insert");
        let second_user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Second",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("second user should insert");
        let second_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Second reply",
                model: Some("gpt-5.5"),
                input_tokens: Some(12),
                output_tokens: Some(6),
                error: false,
                mentions: None,
            })
            .expect("second assistant should insert");
        storage
            .insert_savings_snapshot(&first_assistant.id, Some(10), Some(0.01))
            .expect("first savings should insert");
        storage
            .insert_savings_snapshot_with_crux(
                &second_assistant.id,
                Some(20),
                Some(0.02),
                Some(PricingSource::Headroom),
                7,
            )
            .expect("second savings should insert");
        let cwd = test_dir_path("delete-messages-turn");
        fs::create_dir_all(&cwd).expect("cwd should create");
        storage
            .insert_turn_snapshot(
                &conversation_id,
                &second_user.id,
                "src/lib.rs",
                &cwd.join("src/lib.rs"),
                None,
            )
            .expect("turn snapshot should insert");
        storage
            .upsert_turn_diff(&conversation_id, &second_user.id, &cwd, "diff\n", true)
            .expect("turn diff should insert");

        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            for (id, created_at) in [
                (&first_user.id, "2026-07-03T10:00:00+00:00"),
                (&first_assistant.id, "2026-07-03T10:01:00+00:00"),
                (&second_user.id, "2026-07-03T10:02:00+00:00"),
                (&second_assistant.id, "2026-07-03T10:03:00+00:00"),
            ] {
                connection
                    .execute(
                        "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                        params![created_at, id],
                    )
                    .expect("created_at should update");
            }
        }

        let totals_before = storage.total_savings().expect("savings should total");
        assert_eq!(totals_before.tokens_saved, 37);
        assert_eq!(totals_before.crux_tokens_saved, 7);

        let deleted = storage
            .delete_messages_from(&conversation_id, &second_user.id)
            .expect("messages should delete");
        assert_eq!(deleted, 2);

        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should list");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["First", "First reply"]
        );
        assert_eq!(messages[1].tokens_saved, Some(10));
        let totals_after = storage.total_savings().expect("savings should total");
        assert_eq!(totals_after.tokens_saved, totals_before.tokens_saved);
        assert!((totals_after.savings_usd - totals_before.savings_usd).abs() < 0.000_000_001);
        assert_eq!(
            totals_after.crux_tokens_saved,
            totals_before.crux_tokens_saved
        );
        let turn_diffs = storage
            .list_turn_diffs(&conversation_id)
            .expect("turn diffs should list");
        assert_eq!(turn_diffs.len(), 1);
        assert_eq!(turn_diffs[0].turn_id, second_user.id);
        let turn_snapshots = storage
            .get_turn_snapshots(&conversation_id, &second_user.id)
            .expect("turn snapshots should list");
        assert_eq!(turn_snapshots.len(), 1);

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let snapshot_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM savings_snapshots", [], |row| {
                row.get(0)
            })
            .expect("snapshot count should query");
        assert_eq!(snapshot_count, 1);
        drop(connection);

        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn delete_trailing_assistant_removes_contiguous_tail_and_returns_user() {
        let path = test_db_path("delete-trailing-assistant");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Regenerate this", "gpt-5.5", None)
            .expect("conversation should insert");
        let first_user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "First",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("first user should insert");
        let first_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "First reply",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("first assistant should insert");
        let second_user = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "user",
                content: "Second",
                model: Some("gpt-5.5"),
                input_tokens: None,
                output_tokens: None,
                error: false,
                mentions: None,
            })
            .expect("second user should insert");
        let second_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Second reply",
                model: Some("gpt-5.5"),
                input_tokens: Some(12),
                output_tokens: Some(6),
                error: false,
                mentions: None,
            })
            .expect("second assistant should insert");
        let retry_assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "Retry reply",
                model: Some("gpt-5.5"),
                input_tokens: Some(13),
                output_tokens: Some(7),
                error: false,
                mentions: None,
            })
            .expect("retry assistant should insert");
        storage
            .insert_savings_snapshot_with_crux(
                &retry_assistant.id,
                Some(30),
                Some(0.03),
                Some(PricingSource::Headroom),
                9,
            )
            .expect("retry savings should insert");

        {
            let connection = storage.connection.lock().expect("storage lock poisoned");
            for (id, created_at) in [
                (&first_user.id, "2026-07-03T10:00:00+00:00"),
                (&first_assistant.id, "2026-07-03T10:01:00+00:00"),
                (&second_user.id, "2026-07-03T10:02:00+00:00"),
                (&second_assistant.id, "2026-07-03T10:03:00+00:00"),
                (&retry_assistant.id, "2026-07-03T10:04:00+00:00"),
            ] {
                connection
                    .execute(
                        "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                        params![created_at, id],
                    )
                    .expect("created_at should update");
            }
        }

        let totals_before = storage.total_savings().expect("savings should total");
        assert_eq!(totals_before.tokens_saved, 39);
        assert_eq!(totals_before.crux_tokens_saved, 9);

        let user_content = storage
            .delete_trailing_assistant(&conversation_id)
            .expect("trailing assistants should delete");
        assert_eq!(user_content.as_deref(), Some("Second"));

        let messages = storage
            .get_messages(&conversation_id)
            .expect("messages should list");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            vec!["First", "First reply", "Second"]
        );
        let totals_after = storage.total_savings().expect("savings should total");
        assert_eq!(totals_after.tokens_saved, totals_before.tokens_saved);
        assert!((totals_after.savings_usd - totals_before.savings_usd).abs() < 0.000_000_001);
        assert_eq!(
            totals_after.crux_tokens_saved,
            totals_before.crux_tokens_saved
        );
        assert_eq!(
            storage
                .delete_trailing_assistant(&conversation_id)
                .expect("second delete should be a no-op"),
            None
        );

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let snapshot_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM savings_snapshots", [], |row| {
                row.get(0)
            })
            .expect("snapshot count should query");
        assert_eq!(snapshot_count, 0);
        drop(connection);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_delete_conversation_removes_messages_but_preserves_savings() {
        let path = test_db_path("delete-conversation");
        let storage = Storage::open(&path).expect("storage should open");
        let conversation_id = storage
            .ensure_conversation(None, "Delete this conversation", "gpt-5.5", None)
            .expect("conversation should insert");
        let assistant = storage
            .insert_message(NewMessage {
                conversation_id: &conversation_id,
                role: "assistant",
                content: "This will be deleted",
                model: Some("gpt-5.5"),
                input_tokens: Some(10),
                output_tokens: Some(5),
                error: false,
                mentions: None,
            })
            .expect("assistant message should insert");
        storage
            .insert_savings_snapshot(&assistant.id, Some(20), Some(0.05))
            .expect("savings snapshot should insert");
        let cwd = test_dir_path("delete-turn-diff-cwd");
        fs::create_dir_all(&cwd).expect("cwd should create");
        storage
            .insert_turn_snapshot(
                &conversation_id,
                "turn-delete",
                "file.txt",
                &cwd.join("file.txt"),
                None,
            )
            .expect("turn snapshot should insert");
        storage
            .upsert_turn_diff(&conversation_id, "turn-delete", &cwd, "diff\n", true)
            .expect("turn diff should insert");

        storage
            .delete_conversation(&conversation_id)
            .expect("conversation should delete");

        assert!(storage
            .list_conversations()
            .expect("conversations should list")
            .is_empty());
        assert!(storage
            .get_messages(&conversation_id)
            .expect("messages should list")
            .is_empty());

        let totals = storage.total_savings().expect("savings should total");
        assert_eq!(totals.tokens_saved, 20);
        assert_eq!(totals.savings_usd, 0.05);
        assert_eq!(totals.session_tokens_saved, 20);
        assert_eq!(totals.session_savings_usd, 0.05);
        let today = storage
            .savings_since("0000-01-01T00:00:00Z")
            .expect("archived savings should remain in dated totals");
        assert_eq!(today.tokens_saved, 20);
        assert_eq!(today.savings_usd, 0.05);
        assert_eq!(today.recent_savings, vec![20]);

        let connection = storage.connection.lock().expect("storage lock poisoned");
        let snapshot_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM savings_snapshots", [], |row| {
                row.get(0)
            })
            .expect("snapshot count should query");
        assert_eq!(snapshot_count, 0);
        let archived_snapshot_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM archived_savings_snapshots",
                [],
                |row| row.get(0),
            )
            .expect("archived snapshot count should query");
        assert_eq!(archived_snapshot_count, 1);
        let turn_snapshot_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM turn_snapshots", [], |row| row.get(0))
            .expect("turn snapshot count should query");
        assert_eq!(turn_snapshot_count, 0);
        let turn_diff_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM turn_diffs", [], |row| row.get(0))
            .expect("turn diff count should query");
        assert_eq!(turn_diff_count, 0);

        drop(connection);
        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_file(path);
    }
}
