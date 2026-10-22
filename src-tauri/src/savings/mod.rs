pub(crate) mod dollars;
pub(crate) mod routing;

use crate::storage::Storage;
use chrono::{DateTime, Duration, Local, Utc};
use serde::{Deserialize, Serialize};

const PRICING_PRECEDENCE_SQL: &str = r#"CASE MAX(
    CASE WHEN COALESCE(tokens_saved, 0) > 0 THEN
        CASE pricing_source
            WHEN 'fallback' THEN 3
            WHEN 'model_table' THEN 2
            WHEN 'headroom' THEN 1
        END
    END
)
    WHEN 3 THEN 'fallback'
    WHEN 2 THEN 'model_table'
    WHEN 1 THEN 'headroom'
END"#;

const SAVINGS_EVENTS_SQL: &str = r#"SELECT
    'message' AS event_kind,
    m.id AS event_id,
    NULL AS session_id,
    m.conversation_id,
    m.role,
    m.created_at,
    COALESCE(s.occurred_local_date, date(m.created_at, 'localtime'))
        AS occurred_local_date,
    COALESCE(m.input_tokens, 0) AS input_tokens,
    COALESCE(m.output_tokens, 0) AS output_tokens,
    s.tokens_saved,
    s.savings_usd,
    s.pricing_source,
    s.pricing_source AS recorded_pricing_source,
    COALESCE(s.crux_tokens_saved, 0) AS crux_tokens_saved,
    0 AS cache_read_tokens,
    s.usd_method AS usd_method
FROM messages m
LEFT JOIN savings_snapshots s ON s.message_id = m.id
UNION ALL
SELECT
    'archived_message',
    message_id,
    NULL,
    NULL,
    message_role,
    created_at,
    occurred_local_date,
    0,
    0,
    tokens_saved,
    savings_usd,
    pricing_source,
    pricing_source,
    COALESCE(crux_tokens_saved, 0),
    0,
    usd_method
FROM archived_savings_snapshots
UNION ALL
SELECT
    'agent_turn',
    'terminal-' || t.id,
    t.session_id,
    a.conversation_id,
    'assistant',
    t.created_at,
    t.occurred_local_date,
    COALESCE(t.input_tokens, 0),
    COALESCE(t.output_tokens, 0),
    t.savings_tokens,
    COALESCE(
        t.savings_usd,
        MAX(COALESCE(t.savings_tokens, 0), 0) * ?1
    ),
    COALESCE(
        t.pricing_source,
        CASE WHEN t.savings_tokens > 0 THEN 'fallback' END
    ),
    t.pricing_source,
    COALESCE(t.crux_tokens_saved, 0),
    COALESCE(t.cache_read_tokens, 0),
    t.usd_method
FROM agent_session_turns t
INNER JOIN agent_sessions a ON a.id = t.session_id
WHERE a.mode != 'gui' OR t.has_message_snapshot = 0
UNION ALL
SELECT
    'routing',
    'routing:' || chain_root_id,
    session_id,
    conversation_id,
    'assistant',
    occurred_at,
    occurred_local_date,
    0,
    0,
    0,
    MAX(net_low_usd, 0.0),
    NULL,
    NULL,
    0,
    0,
    'routing_low'
FROM routing_savings
WHERE usage != 'none'"#;

/// The JEV routing dollars of a period (ADR 0016). The dollar sums of the
/// period already hold `low_usd`: the headline uses the low estimate.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoutingTotals {
    /// Settled chains with a price.
    pub chains: i64,
    /// Settled chains that Halv could not price (no measured usage).
    pub unpriced_chains: i64,
    pub low_usd: f64,
    /// The point estimate. Unreported chains have none.
    pub point_usd: f64,
    pub high_usd: f64,
    /// Chains whose `k` came from enough bench pairs.
    pub bench_chains: i64,
    pub unreported_chains: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsTotals {
    pub tokens_saved: i64,
    pub savings_usd: f64,
    /// Dollars saved with the methods of ADR 0015, since `dollars_since`.
    pub dollars_usd: f64,
    /// Tokens saved before the dollar metric (no dollar method).
    pub legacy_tokens_saved: i64,
    /// The date when this install started the dollar metric.
    pub dollars_since: Option<String>,
    /// The version of the price table in use.
    pub price_version: String,
    pub session_dollars_usd: f64,
    pub pending_tokens_saved: i64,
    pub pending_savings_usd: f64,
    /// Unreported dollars with a dollar method.
    pub pending_dollars_usd: f64,
    pub pricing_source: Option<String>,
    pub session_tokens_saved: i64,
    pub session_savings_usd: f64,
    pub session_pricing_source: Option<String>,
    pub crux_tokens_saved: i64,
    pub crux_included_in_total: bool,
    /// The routing part of `dollars_usd`.
    pub routing: RoutingTotals,
    /// The version of the bench calibration of the routing factor `k`.
    pub routing_calibration_version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsToday {
    pub tokens_saved: i64,
    pub savings_usd: f64,
    pub pricing_source: Option<String>,
    pub recent_savings: Vec<i64>,
    /// Dollars saved with the methods of ADR 0015.
    pub dollars_usd: f64,
    /// The dollars of each event in `recent_savings` (0 for old events).
    pub recent_dollars: Vec<f64>,
    /// The routing part of `dollars_usd`.
    pub routing: RoutingTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DailyTokenActivity {
    pub date: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub tokens_saved: i64,
    pub dollars_usd: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionTurnTotalsRow {
    pub session_id: String,
    pub conversation_id: Option<String>,
    pub savings_tokens: u64,
    pub crux_tokens_saved: u64,
    pub cache_read_tokens: u64,
    pub pricing_source: Option<String>,
    pub dollars_usd: f64,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsRange {
    pub since_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavingsReport {
    pub lifetime_totals: SavingsTotals,
    pub today: SavingsToday,
    pub last_seven_days: SavingsToday,
    pub daily_token_activity: Vec<DailyTokenActivity>,
    pub agent_turn_totals: Vec<AgentSessionTurnTotalsRow>,
}

pub fn savings_events_sql() -> &'static str {
    SAVINGS_EVENTS_SQL
}

/// The dollar sums of ADR 0015 count only events with a dollar method.
/// Old events (NULL method) keep their tokens and their legacy dollars,
/// and the UI shows their tokens on a separate line.
const DOLLARS_SQL: &str = "COALESCE(SUM(CASE WHEN usd_method IS NOT NULL
            THEN COALESCE(savings_usd, 0.0) END), 0.0)";
const LEGACY_TOKENS_SQL: &str = "COALESCE(SUM(CASE WHEN usd_method IS NULL
            THEN COALESCE(tokens_saved, 0) END), 0)";

fn savings_sum_sql(filter: &str) -> String {
    let events = savings_events_sql();
    format!(
        "SELECT
            COALESCE(SUM(COALESCE(tokens_saved, 0)), 0),
            COALESCE(SUM(COALESCE(savings_usd, 0.0)), 0.0),
            {PRICING_PRECEDENCE_SQL},
            {DOLLARS_SQL},
            {LEGACY_TOKENS_SQL}
         FROM ({events}) AS savings_events
         {filter}"
    )
}

pub(crate) fn lifetime_savings_sql() -> String {
    savings_sum_sql("")
}

pub(crate) fn session_savings_sql() -> String {
    savings_sum_sql("WHERE created_at >= ?2")
}

pub(crate) fn savings_since_sql() -> String {
    savings_sum_sql(
        "WHERE role = 'assistant'
           AND julianday(created_at) >= julianday(?2)",
    )
}

pub(crate) fn savings_on_local_date_sql() -> String {
    savings_sum_sql(
        "WHERE role = 'assistant'
           AND occurred_local_date = ?2",
    )
}

fn recent_savings_sql(filter: &str) -> String {
    let events = savings_events_sql();
    format!(
        "SELECT tokens_saved,
                CASE WHEN usd_method IS NOT NULL THEN COALESCE(savings_usd, 0.0)
                     ELSE 0.0 END
         FROM ({events}) AS savings_events
         WHERE role = 'assistant'
           AND tokens_saved > 0
           AND {filter}
         ORDER BY created_at DESC, event_id DESC
         LIMIT 24"
    )
}

pub(crate) fn recent_savings_since_sql() -> String {
    recent_savings_sql("julianday(created_at) >= julianday(?2)")
}

pub(crate) fn recent_savings_on_local_date_sql() -> String {
    recent_savings_sql("occurred_local_date = ?2")
}

pub(crate) fn lifetime_crux_savings_sql() -> String {
    let events = savings_events_sql();
    format!(
        "SELECT COALESCE(SUM(COALESCE(crux_tokens_saved, 0)), 0)
         FROM ({events}) AS savings_events"
    )
}

/// The start of the dollar metric: the day migration 0057 ran.
pub(crate) fn dollars_since_sql() -> &'static str {
    "SELECT applied_at FROM migrations WHERE version = '0057_savings_dollars'"
}

pub(crate) fn pending_savings_sql() -> &'static str {
    // The queue keeps the signed `net_usd` for the server. The app counts
    // `savings_usd`, which is never below zero (ADR 0016).
    "SELECT
        COALESCE(SUM(tokens_saved), 0),
        COALESCE(SUM(savings_usd), 0.0),
        COALESCE(SUM(CASE WHEN method IS NOT NULL
                         THEN COALESCE(savings_usd, 0.0) END), 0.0)
     FROM savings_report_queue
     WHERE reported_at IS NULL AND rejected_at IS NULL"
}

pub(crate) fn agent_session_turn_totals_sql() -> String {
    let events = savings_events_sql();
    format!(
        "SELECT session_id,
                conversation_id,
                COALESCE(SUM(MAX(COALESCE(tokens_saved, 0), 0)), 0),
                COALESCE(SUM(MAX(COALESCE(crux_tokens_saved, 0), 0)), 0),
                COALESCE(SUM(MAX(COALESCE(cache_read_tokens, 0), 0)), 0),
                {PRICING_PRECEDENCE_SQL},
                {DOLLARS_SQL}
         FROM (
            SELECT session_id, conversation_id, tokens_saved, crux_tokens_saved,
                   cache_read_tokens, recorded_pricing_source AS pricing_source,
                   savings_usd, usd_method
            FROM ({events}) AS savings_events
            WHERE event_kind = 'agent_turn'
         ) AS agent_events
         GROUP BY session_id"
    )
}

pub(crate) fn daily_token_activity_sql() -> String {
    let events = savings_events_sql();
    format!(
        "SELECT occurred_local_date,
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(
                    CASE WHEN role = 'assistant' THEN tokens_saved ELSE 0 END
                ), 0),
                COALESCE(SUM(
                    CASE WHEN role = 'assistant' AND usd_method IS NOT NULL
                         THEN COALESCE(savings_usd, 0.0) ELSE 0.0 END
                ), 0.0)
         FROM ({events}) AS savings_events
         WHERE occurred_local_date >= date(?2, 'localtime')
         GROUP BY occurred_local_date
         ORDER BY occurred_local_date ASC"
    )
}

/// The local usage of one UTC day for the daily usage summary (ADR 0022).
/// The values are exact. The analytics module rounds them before they leave
/// the device.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct UtcDayUsage {
    /// The stored agent kind of each agent session with activity on the day.
    pub agent_kinds: Vec<String>,
    pub tokens_saved: i64,
    /// The dollars saved with the methods of ADR 0015.
    pub dollars_usd: f64,
    pub crux_tokens_saved: i64,
    /// The routed tasks with a receipt on the day.
    pub routed_tasks: i64,
}

/// The savings of one UTC day, `?2 <= created_at < ?3`. Tokens and dollars
/// count the assistant events, as the savings meter does.
pub(crate) fn utc_day_savings_sql() -> String {
    let events = savings_events_sql();
    format!(
        "SELECT
            COALESCE(SUM(CASE WHEN role = 'assistant'
                         THEN COALESCE(tokens_saved, 0) ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN role = 'assistant' AND usd_method IS NOT NULL
                         THEN COALESCE(savings_usd, 0.0) ELSE 0.0 END), 0.0),
            COALESCE(SUM(COALESCE(crux_tokens_saved, 0)), 0)
         FROM ({events}) AS savings_events
         WHERE julianday(created_at) >= julianday(?2)
           AND julianday(created_at) < julianday(?3)"
    )
}

/// The agent kind of each agent session with activity in one UTC day,
/// `?1 <= time < ?2`. A session counts on the day when it starts, records a
/// turn, or was last active.
pub(crate) fn utc_day_agent_sessions_sql() -> &'static str {
    "SELECT a.agent_kind
     FROM agent_sessions a
     WHERE (julianday(a.created_at) >= julianday(?1)
            AND julianday(a.created_at) < julianday(?2))
        OR (julianday(a.last_active_at) >= julianday(?1)
            AND julianday(a.last_active_at) < julianday(?2))
        OR EXISTS (
            SELECT 1 FROM agent_session_turns t
            WHERE t.session_id = a.id
              AND julianday(t.created_at) >= julianday(?1)
              AND julianday(t.created_at) < julianday(?2)
        )"
}

/// The routed tasks with a receipt in one UTC day, `?1 <= time < ?2`.
pub(crate) fn utc_day_routed_tasks_sql() -> &'static str {
    "SELECT COUNT(*)
     FROM task_route_receipts
     WHERE julianday(reported_at) >= julianday(?1)
       AND julianday(reported_at) < julianday(?2)"
}

pub(crate) fn mcp_session_savings_sql() -> String {
    format!(
        "SELECT
            COALESCE(SUM(MAX(COALESCE(tokens_saved, 0), 0)), 0),
            COUNT(tokens_saved),
            {PRICING_PRECEDENCE_SQL},
            {DOLLARS_SQL}
         FROM (
            SELECT savings_tokens AS tokens_saved, pricing_source, savings_usd,
                   usd_method
            FROM agent_session_turns
            WHERE session_id = ?1
         ) AS session_turns"
    )
}

pub(crate) fn mcp_week_savings_sql() -> String {
    let events = savings_events_sql();
    format!(
        "SELECT
            COALESCE(SUM(MAX(COALESCE(tokens_saved, 0), 0)), 0),
            COALESCE(SUM(MAX(COALESCE(crux_tokens_saved, 0), 0)), 0),
            {PRICING_PRECEDENCE_SQL},
            {DOLLARS_SQL}
         FROM (
            SELECT tokens_saved, crux_tokens_saved, role, created_at,
                   recorded_pricing_source AS pricing_source, savings_usd,
                   usd_method
            FROM ({events}) AS savings_events
         ) AS weekly_events
         WHERE role = 'assistant'
           AND julianday(created_at) >= julianday(?2)"
    )
}

fn trailing_week_cutoff(now: DateTime<Utc>) -> String {
    (now - Duration::days(7)).to_rfc3339()
}

fn range_cutoff(range: &SavingsRange, default_cutoff: &str) -> Result<String, String> {
    let Some(since_ms) = range.since_ms else {
        return Ok(default_cutoff.to_string());
    };
    DateTime::<Utc>::from_timestamp_millis(since_ms)
        .map(|since| since.to_rfc3339())
        .ok_or_else(|| "the savings range start is outside the supported date range".to_string())
}

pub(crate) fn savings_report_at(
    storage: &Storage,
    range: &SavingsRange,
    now: DateTime<Utc>,
) -> Result<SavingsReport, String> {
    let week_cutoff = trailing_week_cutoff(now);
    let activity_cutoff = range_cutoff(range, &week_cutoff)?;
    let local_date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();

    Ok(SavingsReport {
        lifetime_totals: storage.total_savings().map_err(|error| error.to_string())?,
        today: storage
            .savings_on_local_date(&local_date)
            .map_err(|error| error.to_string())?,
        last_seven_days: storage
            .savings_since(&week_cutoff)
            .map_err(|error| error.to_string())?,
        daily_token_activity: storage
            .daily_token_activity(&activity_cutoff)
            .map_err(|error| error.to_string())?,
        agent_turn_totals: storage
            .agent_session_turn_totals()
            .map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
pub async fn savings_report(
    storage: tauri::State<'_, Storage>,
    range: SavingsRange,
) -> Result<SavingsReport, String> {
    savings_report_at(storage.inner(), &range, Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{run_migrations, savings_usd_for_tokens};
    use chrono::TimeZone;
    use rusqlite::{params, Connection};

    fn migrated_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("memory database should open");
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("foreign keys should enable");
        run_migrations(&connection).expect("real migrations should apply");
        connection
    }

    fn insert_conversation(connection: &Connection, id: &str) {
        connection
            .execute(
                "INSERT INTO conversations
                    (id, title, model, created_at, updated_at)
                 VALUES (?1, 'Savings test', 'gpt-5.5', ?2, ?2)",
                params![id, "2026-08-20T00:00:00Z"],
            )
            .expect("conversation should insert");
    }

    struct MessageSnapshotFixture<'a> {
        message_id: &'a str,
        conversation_id: &'a str,
        tokens_saved: i64,
        savings_usd: f64,
        pricing_source: &'a str,
        created_at: &'a str,
        occurred_local_date: &'a str,
    }

    fn insert_message_snapshot(connection: &Connection, fixture: MessageSnapshotFixture<'_>) {
        connection
            .execute(
                "INSERT INTO messages
                    (id, conversation_id, role, content, created_at, model,
                     input_tokens, output_tokens, error)
                 VALUES (?1, ?2, 'assistant', 'Saved response', ?3, 'gpt-5.5',
                         10, 5, 0)",
                params![
                    fixture.message_id,
                    fixture.conversation_id,
                    fixture.created_at
                ],
            )
            .expect("message should insert");
        connection
            .execute(
                "INSERT INTO savings_snapshots
                    (message_id, tokens_saved, savings_usd, pricing_source,
                     crux_tokens_saved, occurred_local_date)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![
                    fixture.message_id,
                    fixture.tokens_saved,
                    fixture.savings_usd,
                    fixture.pricing_source,
                    fixture.occurred_local_date
                ],
            )
            .expect("snapshot should insert");
    }

    struct TurnFixture<'a> {
        session_id: &'a str,
        mode: &'a str,
        tokens_saved: i64,
        savings_usd: Option<f64>,
        pricing_source: Option<&'a str>,
        has_message_snapshot: bool,
        created_at: &'a str,
        occurred_local_date: &'a str,
    }

    fn insert_agent_turn(connection: &Connection, fixture: TurnFixture<'_>) {
        connection
            .execute(
                "INSERT INTO agent_sessions
                    (id, agent_kind, lane, mode, cwd, status, created_at,
                     last_active_at)
                 VALUES (?1, 'codex', 'codex_chatgpt', ?2, '/tmp', 'completed',
                         ?3, ?3)",
                params![fixture.session_id, fixture.mode, fixture.created_at],
            )
            .expect("agent session should insert");
        connection
            .execute(
                "INSERT INTO agent_session_turns
                    (session_id, turn_index, input_tokens, output_tokens,
                     cache_read_tokens, cache_creation_tokens, cost_usd,
                     savings_tokens, savings_usd, pricing_source,
                     crux_tokens_saved, savings_source, has_message_snapshot,
                     created_at, occurred_local_date)
                 VALUES (?1, 0, 20, 10, 3, 0, NULL, ?2, ?3, ?4, 0,
                         'measured', ?5, ?6, ?7)",
                params![
                    fixture.session_id,
                    fixture.tokens_saved,
                    fixture.savings_usd,
                    fixture.pricing_source,
                    i64::from(fixture.has_message_snapshot),
                    fixture.created_at,
                    fixture.occurred_local_date
                ],
            )
            .expect("agent turn should insert");
    }

    fn storage_from(connection: Connection) -> Storage {
        Storage::from_test_connection(connection, "2026-08-20T00:00:00Z")
    }

    #[test]
    fn utc_day_usage_counts_only_the_events_of_that_utc_day() {
        let connection = migrated_connection();
        for (session_id, tokens_saved, created_at) in [
            ("inside", 300, "2026-08-20T23:59:59Z"),
            ("next-day", 900, "2026-08-21T00:00:00Z"),
        ] {
            insert_agent_turn(
                &connection,
                TurnFixture {
                    session_id,
                    mode: "terminal_embedded",
                    tokens_saved,
                    savings_usd: Some(0.4),
                    pricing_source: Some("model_table"),
                    has_message_snapshot: false,
                    created_at,
                    occurred_local_date: &created_at[..10],
                },
            );
        }
        connection
            .execute_batch(
                "UPDATE agent_session_turns
                    SET usd_method = 'input_once', crux_tokens_saved = 7;
                 -- A session of an earlier day with a turn on the day.
                 INSERT INTO agent_sessions
                    (id, agent_kind, lane, mode, cwd, status, created_at,
                     last_active_at)
                 VALUES ('older', 'zai', 'zai', 'terminal_embedded', '/tmp',
                         'completed', '2026-08-18T10:00:00Z',
                         '2026-08-22T10:00:00Z');
                 INSERT INTO agent_session_turns
                    (session_id, turn_index, input_tokens, output_tokens,
                     cache_read_tokens, cache_creation_tokens, savings_tokens,
                     created_at, occurred_local_date)
                 VALUES ('older', 0, 1, 1, 0, 0, 0,
                         '2026-08-20T08:00:00+02:00', '2026-08-20');
                 -- A session without activity on the day.
                 INSERT INTO agent_sessions
                    (id, agent_kind, lane, mode, cwd, status, created_at,
                     last_active_at)
                 VALUES ('idle', 'claude', 'claude', 'terminal_embedded', '/tmp',
                         'completed', '2026-08-18T10:00:00Z',
                         '2026-08-22T10:00:00Z');
                 INSERT INTO task_routes
                    (id, session_id, title, brief_chars, complexity,
                     complexity_confidence, needs_reasoning, risky, tier, lane,
                     reasons_json, status, provider, provider_model, latency_ms,
                     created_at, updated_at)
                 VALUES ('route-1', 'inside', 'Task', 10, 'low', 0.9, 0.1, 0.1,
                         'cheap', 'codex', '[]', 'passed', 'local', 'rules', 1,
                         '2026-08-20T12:00:00Z', '2026-08-20T12:00:00Z');
                 INSERT INTO task_route_receipts
                    (route_id, session_id, outcome, usage_source, reported_at)
                 VALUES ('route-1', 'inside', 'passed', 'none',
                         '2026-08-20T12:30:00Z');",
            )
            .expect("usage fixtures should insert");

        let storage = storage_from(connection);
        let mut usage = storage
            .utc_day_usage("2026-08-20T00:00:00Z", "2026-08-21T00:00:00Z")
            .expect("day usage should load");
        usage.agent_kinds.sort();
        assert_eq!(usage.agent_kinds, vec!["codex", "zai"]);
        assert_eq!(usage.tokens_saved, 300);
        assert!((usage.dollars_usd - 0.4).abs() < 1e-12);
        assert_eq!(usage.crux_tokens_saved, 7);
        assert_eq!(usage.routed_tasks, 1);

        let empty = storage
            .utc_day_usage("2026-08-19T00:00:00Z", "2026-08-20T00:00:00Z")
            .expect("day usage should load");
        assert_eq!(empty, UtcDayUsage::default());
    }

    #[test]
    fn counts_a_message_snapshot_only() {
        let connection = migrated_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message_snapshot(
            &connection,
            MessageSnapshotFixture {
                message_id: "message-1",
                conversation_id: "conversation-1",
                tokens_saved: 100,
                savings_usd: 0.50,
                pricing_source: "headroom",
                created_at: "2026-08-20T10:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );

        let totals = storage_from(connection)
            .total_savings()
            .expect("lifetime totals should load");
        assert_eq!(totals.tokens_saved, 100);
        assert!((totals.savings_usd - 0.50).abs() < 1e-12);
        assert_eq!(totals.pricing_source.as_deref(), Some("headroom"));
    }

    #[test]
    fn counts_an_agent_turn_only() {
        let connection = migrated_connection();
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "terminal-only",
                mode: "terminal_embedded",
                tokens_saved: 200,
                savings_usd: None,
                pricing_source: None,
                has_message_snapshot: false,
                created_at: "2026-08-20T11:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );

        let totals = storage_from(connection)
            .total_savings()
            .expect("lifetime totals should load");
        assert_eq!(totals.tokens_saved, 200);
        assert!((totals.savings_usd - savings_usd_for_tokens(200)).abs() < 1e-12);
        assert_eq!(totals.pricing_source.as_deref(), Some("fallback"));
    }

    #[test]
    fn covered_agent_turn_counts_once_through_its_message_snapshot() {
        let connection = migrated_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message_snapshot(
            &connection,
            MessageSnapshotFixture {
                message_id: "message-1",
                conversation_id: "conversation-1",
                tokens_saved: 100,
                savings_usd: 0.50,
                pricing_source: "headroom",
                created_at: "2026-08-20T10:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "covered-gui",
                mode: "gui",
                tokens_saved: 400,
                savings_usd: Some(2.0),
                pricing_source: Some("model_table"),
                has_message_snapshot: true,
                created_at: "2026-08-20T12:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );

        let totals = storage_from(connection)
            .total_savings()
            .expect("lifetime totals should load");
        assert_eq!(totals.tokens_saved, 100);
        assert!((totals.savings_usd - 0.50).abs() < 1e-12);
        assert_eq!(totals.pricing_source.as_deref(), Some("headroom"));
    }

    #[test]
    fn pricing_precedence_selects_model_price_over_snapshot_price() {
        let connection = migrated_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message_snapshot(
            &connection,
            MessageSnapshotFixture {
                message_id: "message-1",
                conversation_id: "conversation-1",
                tokens_saved: 100,
                savings_usd: 0.50,
                pricing_source: "headroom",
                created_at: "2026-08-20T10:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "model-priced",
                mode: "terminal_embedded",
                tokens_saved: 200,
                savings_usd: Some(0.80),
                pricing_source: Some("model_table"),
                has_message_snapshot: false,
                created_at: "2026-08-20T11:00:00Z",
                occurred_local_date: "2026-08-20",
            },
        );

        let totals = storage_from(connection)
            .total_savings()
            .expect("lifetime totals should load");
        assert_eq!(totals.pricing_source.as_deref(), Some("model_table"));
        assert!((totals.savings_usd - 1.30).abs() < 1e-12);
    }

    #[test]
    fn today_and_since_windows_include_the_boundary() {
        let connection = migrated_connection();
        insert_conversation(&connection, "conversation-1");
        for (id, tokens, created_at, local_date) in [
            ("before", 10, "2026-08-19T23:59:59.999Z", "2026-08-19"),
            ("at", 20, "2026-08-20T00:00:00Z", "2026-08-20"),
            ("after", 30, "2026-08-20T00:00:01Z", "2026-08-20"),
        ] {
            insert_message_snapshot(
                &connection,
                MessageSnapshotFixture {
                    message_id: id,
                    conversation_id: "conversation-1",
                    tokens_saved: tokens,
                    savings_usd: tokens as f64 / 100.0,
                    pricing_source: "headroom",
                    created_at,
                    occurred_local_date: local_date,
                },
            );
        }
        let storage = storage_from(connection);

        let today = storage
            .savings_on_local_date("2026-08-20")
            .expect("local-date totals should load");
        assert_eq!(today.tokens_saved, 50);
        assert_eq!(today.recent_savings, vec![20, 30]);

        let since = storage
            .savings_since("2026-08-20T00:00:00Z")
            .expect("since totals should load");
        assert_eq!(since.tokens_saved, 50);
        assert_eq!(since.recent_savings, vec![20, 30]);
    }

    #[test]
    fn report_matches_legacy_reads_on_one_fixture() {
        let now = Utc
            .with_ymd_and_hms(2026, 8, 20, 12, 0, 0)
            .single()
            .expect("test time should resolve");
        let local_date = now.with_timezone(&Local).format("%Y-%m-%d").to_string();
        let connection = migrated_connection();
        insert_conversation(&connection, "conversation-1");
        insert_message_snapshot(
            &connection,
            MessageSnapshotFixture {
                message_id: "message-1",
                conversation_id: "conversation-1",
                tokens_saved: 100,
                savings_usd: 0.50,
                pricing_source: "headroom",
                created_at: "2026-08-20T10:00:00Z",
                occurred_local_date: &local_date,
            },
        );
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "terminal-only",
                mode: "terminal_embedded",
                tokens_saved: 200,
                savings_usd: Some(0.80),
                pricing_source: Some("model_table"),
                has_message_snapshot: false,
                created_at: "2026-08-20T11:00:00Z",
                occurred_local_date: &local_date,
            },
        );
        let storage = storage_from(connection);
        let range = SavingsRange {
            since_ms: Some(
                Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0)
                    .single()
                    .expect("range time should resolve")
                    .timestamp_millis(),
            ),
        };

        let report =
            savings_report_at(&storage, &range, now).expect("combined savings report should load");
        let week_cutoff = trailing_week_cutoff(now);
        let activity_cutoff = range_cutoff(&range, &week_cutoff).expect("range should resolve");
        assert_eq!(
            report.lifetime_totals,
            storage
                .total_savings()
                .expect("legacy lifetime should load")
        );
        assert_eq!(
            report.today,
            storage
                .savings_on_local_date(&local_date)
                .expect("legacy today should load")
        );
        assert_eq!(
            report.last_seven_days,
            storage
                .savings_since(&week_cutoff)
                .expect("legacy week should load")
        );
        assert_eq!(
            report.daily_token_activity,
            storage
                .daily_token_activity(&activity_cutoff)
                .expect("legacy activity should load")
        );
        assert_eq!(
            report.agent_turn_totals,
            storage
                .agent_session_turn_totals()
                .expect("legacy agent totals should load")
        );
    }

    #[test]
    fn dollar_sums_count_only_rows_with_a_dollar_method() {
        let connection = migrated_connection();
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "legacy",
                mode: "terminal_embedded",
                tokens_saved: 1_000_000,
                savings_usd: Some(1.25),
                pricing_source: Some("fallback"),
                has_message_snapshot: false,
                created_at: "2026-10-15T10:00:00Z",
                occurred_local_date: "2026-10-15",
            },
        );
        insert_agent_turn(
            &connection,
            TurnFixture {
                session_id: "v2",
                mode: "terminal_embedded",
                tokens_saved: 1_000_000,
                savings_usd: Some(0.312),
                pricing_source: Some("model_table"),
                has_message_snapshot: false,
                created_at: "2026-10-15T11:00:00Z",
                occurred_local_date: "2026-10-15",
            },
        );
        connection
            .execute(
                "UPDATE agent_session_turns SET usd_method = 'effective_rate'
                 WHERE session_id = 'v2'",
                [],
            )
            .expect("dollar method should update");
        let storage = storage_from(connection);

        let totals = storage.total_savings().expect("totals should load");
        assert_eq!(totals.tokens_saved, 2_000_000);
        assert_eq!(totals.legacy_tokens_saved, 1_000_000);
        assert!((totals.dollars_usd - 0.312).abs() < 1e-9);
        assert!((totals.savings_usd - 1.562).abs() < 1e-9);
        assert!(totals.dollars_since.is_some());

        let day = storage
            .savings_on_local_date("2026-10-15")
            .expect("day should load");
        assert!((day.dollars_usd - 0.312).abs() < 1e-9);
        assert_eq!(day.recent_savings, vec![1_000_000, 1_000_000]);
        assert_eq!(day.recent_dollars, vec![0.0, 0.312]);

        let sessions = storage
            .agent_session_turn_totals()
            .expect("session totals should load");
        let v2 = sessions
            .iter()
            .find(|row| row.session_id == "v2")
            .expect("v2 session should have totals");
        assert!((v2.dollars_usd - 0.312).abs() < 1e-9);
    }
}
