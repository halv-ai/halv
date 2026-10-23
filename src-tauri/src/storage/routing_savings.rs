//! Schema and queries of the JEV routing dollars (ADR 0016, migration
//! 0058_routing_savings).
//!
//! Rules:
//! - Halv settles a chain once, when no attempt is still `routed`. The row
//!   keeps the estimator, calibration and price versions that settled it. A
//!   later calibration does not change a settled row.
//! - Only routes of this build count: a chain whose root is older than the
//!   migration stays unsettled (Pedro, 2026-10-15: no restatement of old
//!   rows).
//! - A settled chain with a price queues one report event,
//!   `routing:<root id>`, with 0 tokens and its low estimate as `net_usd`.

use super::{
    list_task_route_agent_rows, list_task_routes_rows, now_string, replace_task_route_agent_rows,
    Storage, StorageResult,
};
use crate::pricing::calibration::Calibration;
use crate::savings::routing::{self as estimator, Attempt, ChainUsage, Settlement};
use crate::savings::RoutingTotals;
use chrono::{DateTime, Local};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub(super) const MIGRATION_ROUTING_SAVINGS: &str = "0058_routing_savings";

const ROUTING_SAVINGS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS routing_savings (
    chain_root_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    conversation_id TEXT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'unreported')),
    attempts INTEGER NOT NULL,
    usage TEXT NOT NULL CHECK (usage IN ('measured', 'no_worker', 'none')),
    default_model TEXT NULL,
    routed_models TEXT NULL,
    actual_usd REAL NOT NULL,
    overhead_usd REAL NOT NULL,
    counterfactual_low_usd REAL NOT NULL,
    counterfactual_point_usd REAL NULL,
    counterfactual_high_usd REAL NOT NULL,
    net_low_usd REAL NOT NULL,
    net_point_usd REAL NULL,
    net_high_usd REAL NOT NULL,
    k_low REAL NULL,
    k_point REAL NULL,
    k_high REAL NULL,
    k_source TEXT NOT NULL
        CHECK (k_source IN ('exact', 'floor', 'thin', 'bench', 'none')),
    estimator_version TEXT NOT NULL,
    calibration_version TEXT NOT NULL,
    price_version TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    occurred_local_date TEXT NOT NULL,
    settled_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS routing_savings_session_idx
    ON routing_savings(session_id);

CREATE INDEX IF NOT EXISTS routing_savings_conversation_idx
    ON routing_savings(conversation_id);

CREATE INDEX IF NOT EXISTS routing_savings_occurred_idx
    ON routing_savings(occurred_at);
"#;

pub(super) fn apply_routing_savings_migration(connection: &Connection) -> StorageResult<()> {
    connection.execute_batch(ROUTING_SAVINGS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_ROUTING_SAVINGS, now_string()],
    )?;
    Ok(())
}

/// The settled savings of one chain, for the task board.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainSavingsRow {
    pub chain_root_id: String,
    pub outcome: String,
    pub usage: String,
    pub default_model: Option<String>,
    pub actual_usd: f64,
    pub net_low_usd: f64,
    pub net_point_usd: Option<f64>,
    pub net_high_usd: f64,
    pub k_source: String,
}

/// The measured cost of one route, for the task board.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteCostRow {
    pub route_id: String,
    pub actual_usd: f64,
    pub models: Vec<String>,
}

/// The routing dollars of one session or conversation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingSavingsReport {
    pub totals: RoutingTotals,
    pub chains: Vec<ChainSavingsRow>,
    pub route_costs: Vec<RouteCostRow>,
}

/// The part of the routing table that a total covers.
#[derive(Debug, Clone, Copy)]
pub(crate) enum RoutingScope<'a> {
    All,
    Since(&'a str),
    LocalDate(&'a str),
    Session(&'a str),
    Conversation(&'a str),
}

impl RoutingScope<'_> {
    fn filter(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::All => ("1 = 1", None),
            Self::Since(since) => ("julianday(occurred_at) >= julianday(?1)", Some(since)),
            Self::LocalDate(date) => ("occurred_local_date = ?1", Some(date)),
            Self::Session(id) => ("session_id = ?1", Some(id)),
            Self::Conversation(id) => ("conversation_id = ?1", Some(id)),
        }
    }
}

pub(crate) fn routing_totals(
    connection: &Connection,
    scope: RoutingScope<'_>,
) -> rusqlite::Result<RoutingTotals> {
    let (filter, value) = scope.filter();
    // A chain counts at most zero loss (ADR 0016). The signed estimate
    // stays in the row for the details.
    // A floor or thin chain has no bench evidence for its upper values.
    // Its point and high end count as its low estimate (ADR 0016,
    // "Display"). The row keeps the stored values for audit.
    let sql = format!(
        "SELECT
            COUNT(CASE WHEN usage != 'none' THEN 1 END),
            COUNT(CASE WHEN usage = 'none' THEN 1 END),
            COALESCE(SUM(CASE WHEN usage != 'none' THEN MAX(net_low_usd, 0.0) END), 0.0),
            COALESCE(SUM(CASE WHEN usage != 'none' THEN MAX(
                CASE WHEN k_source IN ('floor', 'thin') THEN net_low_usd
                     ELSE net_point_usd END, 0.0) END), 0.0),
            COALESCE(SUM(CASE WHEN usage != 'none' THEN MAX(
                CASE WHEN k_source IN ('floor', 'thin') THEN net_low_usd
                     ELSE net_high_usd END, 0.0) END), 0.0),
            COUNT(CASE WHEN usage != 'none' AND k_source = 'bench' THEN 1 END),
            COUNT(CASE WHEN usage != 'none' AND outcome = 'unreported' THEN 1 END)
         FROM routing_savings WHERE {filter}"
    );
    let map = |row: &rusqlite::Row<'_>| {
        Ok(RoutingTotals {
            chains: row.get(0)?,
            unpriced_chains: row.get(1)?,
            low_usd: row.get(2)?,
            point_usd: row.get(3)?,
            high_usd: row.get(4)?,
            bench_chains: row.get(5)?,
            unreported_chains: row.get(6)?,
        })
    };
    match value {
        Some(value) => connection.query_row(&sql, params![value], map),
        None => connection.query_row(&sql, [], map),
    }
}

/// The roots of chains that started after the migration and have no
/// settlement yet.
fn unsettled_roots(connection: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT r.id FROM task_routes r
         WHERE r.escalated_from_id IS NULL
           AND julianday(r.created_at) >= julianday((
               SELECT applied_at FROM migrations WHERE version = ?1))
           AND NOT EXISTS (
               SELECT 1 FROM routing_savings s WHERE s.chain_root_id = r.id)
         ORDER BY r.created_at ASC, r.id ASC",
    )?;
    let roots = statement
        .query_map(params![MIGRATION_ROUTING_SAVINGS], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(roots)
}

fn batch_size(connection: &Connection, batch_id: &str) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM task_routes WHERE batch_id = ?1 AND escalated_from_id IS NULL",
        params![batch_id],
        |row| row.get(0),
    )
}

/// The attempts of the chain that starts at `root_id`, root first.
fn load_chain(connection: &Connection, root_id: &str) -> rusqlite::Result<Vec<Attempt>> {
    let mut chain = Vec::new();
    let mut next = list_task_routes_rows(connection, "id = ?1", root_id)?.pop();
    // A chain is short. The limit stops a loop in damaged data.
    while let Some(route) = next.take() {
        if chain.len() >= 32 {
            break;
        }
        let agents = list_task_route_agent_rows(connection, &route.id)?;
        let batch = match &route.workflow {
            Some(workflow) => batch_size(connection, &workflow.batch_id)?,
            None => 1,
        };
        next = list_task_routes_rows(connection, "escalated_from_id = ?1", &route.id)?
            .into_iter()
            .next();
        chain.push(Attempt {
            route,
            agents,
            batch_size: batch,
        });
    }
    Ok(chain)
}

fn occurred(chain: &[Attempt]) -> (String, String) {
    let at = chain
        .iter()
        .map(|attempt| attempt.route.updated_at.as_str())
        .max()
        .map(str::to_string)
        .unwrap_or_else(now_string);
    let local_date = DateTime::parse_from_rfc3339(&at)
        .map(|time| time.with_timezone(&Local))
        .unwrap_or_else(|_| Local::now())
        .format("%Y-%m-%d")
        .to_string();
    (at, local_date)
}

fn insert_settlement(
    connection: &Connection,
    settlement: &Settlement,
    calibration_version: &str,
    occurred_at: &str,
    occurred_local_date: &str,
) -> rusqlite::Result<bool> {
    let inserted = connection.execute(
        "INSERT OR IGNORE INTO routing_savings (
            chain_root_id, session_id, conversation_id, outcome, attempts, usage,
            default_model, routed_models, actual_usd, overhead_usd,
            counterfactual_low_usd, counterfactual_point_usd, counterfactual_high_usd,
            net_low_usd, net_point_usd, net_high_usd, k_low, k_point, k_high,
            k_source, estimator_version, calibration_version, price_version,
            occurred_at, occurred_local_date, settled_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                   ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
        params![
            settlement.chain_root_id,
            settlement.session_id,
            settlement.conversation_id,
            settlement.outcome,
            settlement.attempts,
            settlement.usage.as_str(),
            settlement.default_model,
            (!settlement.routed_models.is_empty()).then(|| settlement.routed_models.join("\n")),
            settlement.actual_usd,
            settlement.overhead_usd,
            settlement.counterfactual_low_usd,
            settlement.counterfactual_point_usd,
            settlement.counterfactual_high_usd,
            settlement.net_low_usd,
            settlement.net_point_usd,
            settlement.net_high_usd,
            settlement.k_low,
            settlement.k_point,
            settlement.k_high,
            settlement.k_source,
            estimator::ESTIMATOR_VERSION,
            calibration_version,
            settlement.price_version,
            occurred_at,
            occurred_local_date,
            now_string(),
        ],
    )?;
    Ok(inserted > 0)
}

/// Queues the routing event of a settled chain. `savings_usd` is never
/// negative, because old servers refuse a negative value.
fn enqueue_routing_event(connection: &Connection, settlement: &Settlement) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO savings_report_queue
            (message_id, tokens_saved, savings_usd, created_at, reported_at,
             kind, net_usd, method, price_version)
         VALUES (?1, 0, ?2, ?3, NULL, 'routing', ?4, ?5, ?6)
         ON CONFLICT(message_id) DO NOTHING",
        params![
            format!("routing:{}", settlement.chain_root_id),
            settlement.net_low_usd.max(0.0),
            now_string(),
            settlement.net_low_usd,
            estimator::METHOD_CHAIN_LOW,
            settlement.price_version,
        ],
    )?;
    Ok(())
}

/// Settles every chain that can settle. Returns the new settlements.
pub(crate) fn settle_pending_chains(
    connection: &Connection,
    calibration: &Calibration,
) -> rusqlite::Result<Vec<Settlement>> {
    let mut settled = Vec::new();
    for root in unsettled_roots(connection)? {
        let chain = load_chain(connection, &root)?;
        let Some(settlement) = estimator::settle_chain(&chain, calibration) else {
            continue;
        };
        let (occurred_at, local_date) = occurred(&chain);
        if !insert_settlement(
            connection,
            &settlement,
            &calibration.calibration_version,
            &occurred_at,
            &local_date,
        )? {
            continue;
        }
        if settlement.usage != ChainUsage::None {
            enqueue_routing_event(connection, &settlement)?;
        }
        settled.push(settlement);
    }
    Ok(settled)
}

/// Workflow routes that a session end closed before any report. Their
/// agents ran, but no report measured them.
fn unmeasured_unreported_routes(
    connection: &Connection,
) -> rusqlite::Result<Vec<(super::TaskRoute, String)>> {
    let mut statement = connection.prepare(
        "SELECT r.id, a.cli_session_id FROM task_routes r
         INNER JOIN agent_sessions a ON a.id = r.session_id
         WHERE r.closed_unreported = 1
           AND r.batch_id IS NOT NULL
           AND a.cli_session_id IS NOT NULL
           AND julianday(r.created_at) >= julianday((
               SELECT applied_at FROM migrations WHERE version = ?1))
           AND NOT EXISTS (SELECT 1 FROM task_route_agents g WHERE g.route_id = r.id)
           AND NOT EXISTS (
               SELECT 1 FROM routing_savings s WHERE s.chain_root_id = r.id)",
    )?;
    let ids = statement
        .query_map(params![MIGRATION_ROUTING_SAVINGS], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut routes = Vec::new();
    for (id, cli_session_id) in ids {
        if let Some(route) = list_task_routes_rows(connection, "id = ?1", &id)?.pop() {
            routes.push((route, cli_session_id));
        }
    }
    Ok(routes)
}

fn chain_rows(
    connection: &Connection,
    scope: RoutingScope<'_>,
) -> rusqlite::Result<Vec<ChainSavingsRow>> {
    let (filter, value) = scope.filter();
    let mut statement = connection.prepare(&format!(
        "SELECT chain_root_id, outcome, usage, default_model, actual_usd, net_low_usd,
                net_point_usd, net_high_usd, k_source
         FROM routing_savings WHERE {filter}
         ORDER BY occurred_at ASC, chain_root_id ASC"
    ))?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(ChainSavingsRow {
            chain_root_id: row.get(0)?,
            outcome: row.get(1)?,
            usage: row.get(2)?,
            default_model: row.get(3)?,
            actual_usd: row.get(4)?,
            net_low_usd: row.get(5)?,
            net_point_usd: row.get(6)?,
            net_high_usd: row.get(7)?,
            k_source: row.get(8)?,
        })
    };
    let rows = match value {
        Some(value) => statement.query_map(params![value], map)?.collect(),
        None => statement.query_map([], map)?.collect(),
    };
    rows
}

fn route_cost_rows(
    connection: &Connection,
    route_filter: &str,
    value: &str,
) -> rusqlite::Result<Vec<RouteCostRow>> {
    let mut statement = connection.prepare(&format!(
        "SELECT g.route_id, g.observed_model, g.input_tokens, g.output_tokens,
                g.cache_read_tokens, g.cache_write_5m_tokens, g.cache_write_1h_tokens,
                g.actual_usd, r.lane
         FROM task_route_agents g
         INNER JOIN task_routes r ON r.id = g.route_id
         WHERE {route_filter}
         ORDER BY g.route_id, g.observed_model"
    ))?;
    let mut rows: Vec<RouteCostRow> = Vec::new();
    let mut query = statement.query(params![value])?;
    while let Some(row) = query.next()? {
        let route_id: String = row.get(0)?;
        let model: String = row.get(1)?;
        let count = |index: usize| -> rusqlite::Result<u64> {
            Ok(u64::try_from(row.get::<_, i64>(index)?).unwrap_or_default())
        };
        let lane: String = row.get(8)?;
        let usd = row
            .get::<_, Option<f64>>(7)?
            .filter(|usd| usd.is_finite() && *usd >= 0.0)
            .unwrap_or_else(|| {
                let usage = crate::pricing::TokenUsage {
                    input: count(2).unwrap_or_default(),
                    output: count(3).unwrap_or_default(),
                    cache_read: count(4).unwrap_or_default(),
                    cache_write_5m: count(5).unwrap_or_default(),
                    cache_write_1h: count(6).unwrap_or_default(),
                };
                crate::pricing::resolve(Some(&lane), Some(&model))
                    .card
                    .cost_usd(&usage, None)
            });
        match rows.last_mut().filter(|last| last.route_id == route_id) {
            Some(last) => {
                last.actual_usd += usd;
                if !last.models.contains(&model) {
                    last.models.push(model);
                }
            }
            None => rows.push(RouteCostRow {
                route_id,
                actual_usd: usd,
                models: vec![model],
            }),
        }
    }
    Ok(rows)
}

impl Storage {
    /// Settles the chains that can settle and queues their routing events.
    /// A workflow route that a session end closed gets its agents measured
    /// first, so the low estimate can count their cost as a failure.
    pub(crate) fn settle_routing_chains(&self) -> StorageResult<usize> {
        let unreported = {
            let connection = self.connection.lock().expect("storage lock poisoned");
            unmeasured_unreported_routes(&connection)?
        };
        // Read the transcripts without the storage lock.
        let measured = if unreported.is_empty() {
            Vec::new()
        } else {
            crate::savings_mcp::measure_unreported_route_agents(&unreported)
        };
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        for (route_id, agents) in &measured {
            let still_empty = transaction
                .query_row(
                    "SELECT 1 FROM task_route_agents WHERE route_id = ?1 LIMIT 1",
                    params![route_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none();
            if still_empty && !agents.is_empty() {
                replace_task_route_agent_rows(&transaction, route_id, agents)?;
            }
        }
        let settled = settle_pending_chains(&transaction, crate::pricing::calibration::current())?;
        transaction.commit()?;
        Ok(settled.len())
    }

    /// The routing dollars of one session or one conversation.
    pub(crate) fn routing_savings_report(
        &self,
        session_id: Option<&str>,
        conversation_id: Option<&str>,
    ) -> StorageResult<RoutingSavingsReport> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let (scope, route_filter, value) = match (conversation_id, session_id) {
            (Some(id), _) => (RoutingScope::Conversation(id), "r.conversation_id = ?1", id),
            (None, Some(id)) => (RoutingScope::Session(id), "r.session_id = ?1", id),
            (None, None) => {
                return Ok(RoutingSavingsReport {
                    totals: routing_totals(&connection, RoutingScope::All)?,
                    chains: Vec::new(),
                    route_costs: Vec::new(),
                })
            }
        };
        Ok(RoutingSavingsReport {
            totals: routing_totals(&connection, scope)?,
            chains: chain_rows(&connection, scope)?,
            route_costs: route_cost_rows(&connection, route_filter, value)?,
        })
    }
}

#[tauri::command]
pub async fn storage_routing_savings(
    storage: tauri::State<'_, Storage>,
    session_id: Option<String>,
    conversation_id: Option<String>,
) -> Result<RoutingSavingsReport, String> {
    let storage = storage.inner();
    storage
        .settle_routing_chains()
        .map_err(|error| error.to_string())?;
    storage
        .routing_savings_report(session_id.as_deref(), conversation_id.as_deref())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::{insert_task_route_row, run_migrations, TaskRoute, TaskRouteAgent};
    use super::*;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().expect("memory database should open");
        run_migrations(&connection).expect("migrations should run");
        // Put the migration in the past, so test routes count.
        connection
            .execute(
                "UPDATE migrations SET applied_at = '2026-10-15T00:00:00Z' WHERE version = ?1",
                params![MIGRATION_ROUTING_SAVINGS],
            )
            .unwrap();
        connection
    }

    fn route(id: &str, status: &str, created_at: &str) -> TaskRoute {
        TaskRoute {
            id: id.to_string(),
            session_id: "session".to_string(),
            conversation_id: Some("conversation".to_string()),
            title: "Task".to_string(),
            brief_chars: 400,
            complexity: "standard".to_string(),
            complexity_confidence: 0.9,
            needs_reasoning: 0.2,
            risky: 0.1,
            tier: "cheapest".to_string(),
            lane: "claude".to_string(),
            model: Some("haiku".to_string()),
            effort: Some("low".to_string()),
            reasons: Vec::new(),
            worker_lane: "claude".to_string(),
            status: status.to_string(),
            escalated_from_id: None,
            provider: "jev".to_string(),
            provider_model: "jev".to_string(),
            latency_ms: 10,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            default_model: Some("claude-opus-5-5".to_string()),
            default_effort: Some("xhigh".to_string()),
            default_source: Some("observed".to_string()),
            closed_unreported: false,
            workflow: None,
            receipt: None,
        }
    }

    fn haiku_agent(route_id: &str) -> TaskRouteAgent {
        TaskRouteAgent {
            route_id: route_id.to_string(),
            agent_id: "agent".to_string(),
            observed_model: "claude-haiku-5-5".to_string(),
            workflow_run_id: "run".to_string(),
            label: "jev:x:read".to_string(),
            observed_effort: Some("low".to_string()),
            input_tokens: 0,
            output_tokens: 10_000,
            cache_read_tokens: 1_000_000,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            actual_usd: Some(0.015),
            price_version: Some("2026-10-15".to_string()),
            started_at: None,
            ended_at: None,
            observed_at: "2026-10-16T10:05:00Z".to_string(),
        }
    }

    #[test]
    fn a_passed_haiku_route_settles_once_and_queues_one_event() {
        let connection = connection();
        insert_task_route_row(&connection, &route("a", "passed", "2026-10-16T10:00:00Z")).unwrap();
        replace_task_route_agent_rows(&connection, "a", &[haiku_agent("a")]).unwrap();
        let calibration = crate::pricing::calibration::current();

        let settled = settle_pending_chains(&connection, calibration).unwrap();
        assert_eq!(settled.len(), 1);
        assert!(settled[0].net_low_usd > 0.0);
        assert!(settle_pending_chains(&connection, calibration)
            .unwrap()
            .is_empty());

        let (kind, tokens, net, savings): (String, i64, f64, f64) = connection
            .query_row(
                "SELECT kind, tokens_saved, net_usd, savings_usd FROM savings_report_queue
                 WHERE message_id = 'routing:a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(kind, "routing");
        assert_eq!(tokens, 0);
        assert!((net - settled[0].net_low_usd).abs() < 1e-12);
        assert!((savings - net).abs() < 1e-12);

        let totals =
            routing_totals(&connection, RoutingScope::Conversation("conversation")).unwrap();
        assert_eq!(totals.chains, 1);
        assert!((totals.low_usd - settled[0].net_low_usd).abs() < 1e-12);
        assert!(totals.high_usd >= totals.low_usd);
        // No cell has bench evidence for this pair, so the range collapses.
        assert_eq!(settled[0].k_source, "floor");
        assert!(settled[0].net_high_usd > settled[0].net_low_usd);
        assert_eq!(totals.high_usd, totals.low_usd);
    }

    /// Inserts one settled row with the given source and net values.
    fn insert_chain(
        connection: &Connection,
        id: &str,
        outcome: &str,
        k_source: &str,
        net: (f64, Option<f64>, f64),
    ) {
        connection
            .execute(
                "INSERT INTO routing_savings (
                    chain_root_id, session_id, conversation_id, outcome, attempts, usage,
                    default_model, routed_models, actual_usd, overhead_usd,
                    counterfactual_low_usd, counterfactual_point_usd, counterfactual_high_usd,
                    net_low_usd, net_point_usd, net_high_usd, k_low, k_point, k_high,
                    k_source, estimator_version, calibration_version, price_version,
                    occurred_at, occurred_local_date, settled_at
                 ) VALUES (?1, 'session', 'conversation', ?2, 1, 'measured',
                           'claude-fable-5-1', 'claude-sonnet-5-5', 1.0, 0.01,
                           1.0, 1.0, 4.0, ?3, ?4, ?5, 0.25, 0.25, 1.0,
                           ?6, 'jev-chain-1', 'test', '2026-10-15',
                           '2026-10-16T10:00:00Z', '2026-10-16', '2026-10-16T10:00:00Z')",
                params![id, outcome, net.0, net.1, net.2, k_source],
            )
            .unwrap();
    }

    #[test]
    fn floor_and_thin_chains_count_their_low_estimate_as_the_high_end() {
        let connection = connection();
        let chains = [
            ("floor", "passed", "floor", (0.5, Some(0.5), 9.0)),
            ("thin", "passed", "thin", (0.25, Some(0.4), 3.0)),
            ("open", "unreported", "floor", (-1.0, None, 2.0)),
        ];
        for (id, outcome, k_source, net) in chains {
            insert_chain(&connection, id, outcome, k_source, net);
        }
        let totals = routing_totals(&connection, RoutingScope::All).unwrap();
        assert_eq!(totals.chains, 3);
        assert!((totals.low_usd - 0.75).abs() < 1e-12);
        assert!((totals.point_usd - 0.75).abs() < 1e-12);
        assert!((totals.high_usd - 0.75).abs() < 1e-12);
    }

    #[test]
    fn a_calibrated_chain_keeps_its_range_next_to_a_floor_chain() {
        let connection = connection();
        let chains = [
            ("bench", "passed", "bench", (0.5, Some(1.0), 2.0)),
            ("floor", "passed", "floor", (0.25, Some(0.25), 9.0)),
            ("exact", "passed", "exact", (-0.1, Some(-0.1), -0.1)),
        ];
        for (id, outcome, k_source, net) in chains {
            insert_chain(&connection, id, outcome, k_source, net);
        }
        let totals = routing_totals(&connection, RoutingScope::All).unwrap();
        assert_eq!(totals.chains, 3);
        assert_eq!(totals.bench_chains, 1);
        assert!((totals.low_usd - 0.75).abs() < 1e-12);
        assert!((totals.point_usd - 1.25).abs() < 1e-12);
        assert!((totals.high_usd - 2.25).abs() < 1e-12);
    }

    #[test]
    fn the_dollar_totals_hold_the_routing_low_estimate() {
        let connection = connection();
        insert_task_route_row(&connection, &route("a", "passed", "2026-10-16T10:00:00Z")).unwrap();
        replace_task_route_agent_rows(&connection, "a", &[haiku_agent("a")]).unwrap();
        let settled =
            settle_pending_chains(&connection, crate::pricing::calibration::current()).unwrap();
        let low = settled[0].net_low_usd;
        let storage = Storage::from_test_connection(connection, "2026-10-16T00:00:00Z");
        let totals = storage.total_savings().unwrap();
        assert_eq!(totals.routing.chains, 1);
        assert!((totals.dollars_usd - low).abs() < 1e-12);
        assert_eq!(totals.tokens_saved, 0);
        let week = storage.savings_since("2026-10-15T00:00:00Z").unwrap();
        assert!((week.dollars_usd - low).abs() < 1e-12);
        assert!(week.recent_savings.is_empty());
        // The pending dollars count the floored event value.
        assert!((totals.pending_dollars_usd - low).abs() < 1e-12);
    }

    #[test]
    fn routes_before_the_migration_never_settle() {
        let connection = connection();
        insert_task_route_row(&connection, &route("old", "passed", "2026-10-01T10:00:00Z"))
            .unwrap();
        let settled =
            settle_pending_chains(&connection, crate::pricing::calibration::current()).unwrap();
        assert!(settled.is_empty());
    }

    #[test]
    fn a_routed_chain_waits_and_a_loss_reports_a_zero_savings_usd() {
        let connection = connection();
        insert_task_route_row(&connection, &route("a", "routed", "2026-10-16T10:00:00Z")).unwrap();
        let calibration = crate::pricing::calibration::current();
        assert!(settle_pending_chains(&connection, calibration)
            .unwrap()
            .is_empty());

        connection
            .execute(
                "UPDATE task_routes SET status = 'failed' WHERE id = 'a'",
                [],
            )
            .unwrap();
        replace_task_route_agent_rows(&connection, "a", &[haiku_agent("a")]).unwrap();
        let settled = settle_pending_chains(&connection, calibration).unwrap();
        assert_eq!(settled[0].outcome, "failed");
        assert!(settled[0].net_low_usd < 0.0);
        let (net, savings): (f64, f64) = connection
            .query_row(
                "SELECT net_usd, savings_usd FROM savings_report_queue
                 WHERE message_id = 'routing:a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(net < 0.0);
        assert_eq!(savings, 0.0);
        // The loss stays in the row. Every counted total floors it at zero.
        let totals = routing_totals(&connection, RoutingScope::All).unwrap();
        assert_eq!(totals.chains, 1);
        assert_eq!(totals.low_usd, 0.0);
        assert_eq!(totals.high_usd, 0.0);
        let storage = Storage::from_test_connection(connection, "2026-10-16T00:00:00Z");
        let totals = storage.total_savings().unwrap();
        assert_eq!(totals.dollars_usd, 0.0);
        assert_eq!(totals.pending_dollars_usd, 0.0);
    }

    #[test]

    fn a_max_route_counts_no_loss_in_the_totals() {
        let connection = connection();

        let mut route = route("a", "passed", "2026-10-16T10:00:00Z");

        route.model = None;

        route.effort = None;

        route.tier = "max".to_string();

        insert_task_route_row(&connection, &route).unwrap();

        let settled =
            settle_pending_chains(&connection, crate::pricing::calibration::current()).unwrap();

        assert_eq!(settled[0].usage, ChainUsage::NoWorker);

        assert!(settled[0].net_low_usd < 0.0);

        let totals = routing_totals(&connection, RoutingScope::All).unwrap();

        assert_eq!(totals.chains, 1);

        assert_eq!(totals.low_usd, 0.0);

        assert_eq!(totals.high_usd, 0.0);
    }

    #[test]
    fn an_unpriced_chain_counts_apart_and_queues_nothing() {
        let connection = connection();
        insert_task_route_row(&connection, &route("a", "passed", "2026-10-16T10:00:00Z")).unwrap();
        let settled =
            settle_pending_chains(&connection, crate::pricing::calibration::current()).unwrap();
        assert_eq!(settled[0].usage, ChainUsage::None);
        let queued: i64 = connection
            .query_row("SELECT COUNT(*) FROM savings_report_queue", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(queued, 0);
        let totals = routing_totals(&connection, RoutingScope::All).unwrap();
        assert_eq!(totals.chains, 0);
        assert_eq!(totals.unpriced_chains, 1);
    }

    #[test]
    fn an_escalation_settles_with_its_root() {
        let connection = connection();
        insert_task_route_row(&connection, &route("a", "failed", "2026-10-16T10:00:00Z")).unwrap();
        let mut retry = route("b", "passed", "2026-10-16T10:10:00Z");
        retry.escalated_from_id = Some("a".to_string());
        retry.model = None;
        retry.effort = None;
        retry.tier = "max".to_string();
        insert_task_route_row(&connection, &retry).unwrap();
        replace_task_route_agent_rows(&connection, "a", &[haiku_agent("a")]).unwrap();
        let settled =
            settle_pending_chains(&connection, crate::pricing::calibration::current()).unwrap();
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].chain_root_id, "a");
        assert_eq!(settled[0].attempts, 2);
        assert_eq!(settled[0].outcome, "passed");
        assert!((settled[0].net_low_usd + 0.015 + settled[0].overhead_usd).abs() < 1e-12);
    }
}
