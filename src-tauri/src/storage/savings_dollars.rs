//! Schema and queries of the dollar savings metric (ADR 0015, migration
//! 0057_savings_dollars).
//!
//! Rules:
//! - Each step of the migration is idempotent. The migration runs its steps
//!   on every start, so a build that had an earlier, shorter 0057 gets the
//!   later steps too.
//! - A rejected savings report is a dead letter. The reporter never sends it
//!   again, and the pending totals do not count it.

use super::{
    column_exists, delete_unreported_report_parts, now_string, report_event_parts, Storage,
    StorageResult,
};
use rusqlite::{params, Connection};

pub(super) const MIGRATION_SAVINGS_DOLLARS: &str = "0057_savings_dollars";

/// The dollar fields of one saved agent turn.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnDollars<'a> {
    pub model: Option<&'a str>,
    pub usd_method: Option<&'a str>,
    pub price_version: Option<&'a str>,
    pub prompt_rate_usd_per_m: Option<f64>,
    pub source_kind: Option<&'a str>,
    pub cache_write_1h_tokens: i64,
}

/// The dollar fields of one saved message (ADR 0015).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapshotDollars<'a> {
    pub usd: f64,
    pub source: crate::pricing::PricingSource,
    pub usd_method: &'a str,
    pub price_version: &'a str,
}

/// The optional dollar fields of one report event. Old servers ignore them.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct ReportDollars {
    /// `context`, `routing` or `output_estimate`.
    pub kind: String,
    /// The signed dollar value. `savings_usd` is never negative, because
    /// old servers refuse a negative value.
    pub net_usd: f64,
    pub method: String,
    pub price_version: String,
}

/// Rebuilds the report queue. A routing event saves no tokens, so the
/// CHECK allows 0 now (was > 0). The rebuild keeps every row and its
/// `reported_at`.
fn rebuild_report_queue(connection: &Connection) -> StorageResult<()> {
    let has_dead_letters = column_exists(connection, "savings_report_queue", "rejected_at")?;
    let (rejected_at, rejected_reason) = if has_dead_letters {
        ("rejected_at", "rejected_reason")
    } else {
        ("NULL", "NULL")
    };
    connection.execute_batch(&format!(
        "CREATE TABLE savings_report_queue_v2 (
            message_id TEXT PRIMARY KEY,
            tokens_saved INTEGER NOT NULL CHECK (tokens_saved >= 0),
            savings_usd REAL,
            created_at TEXT NOT NULL,
            reported_at TEXT,
            kind TEXT NOT NULL DEFAULT 'context'
                CHECK (kind IN ('context', 'routing', 'output_estimate')),
            net_usd REAL NULL,
            method TEXT NULL,
            price_version TEXT NULL,
            rejected_at TEXT NULL,
            rejected_reason TEXT NULL
        );
        INSERT INTO savings_report_queue_v2
            (message_id, tokens_saved, savings_usd, created_at, reported_at,
             rejected_at, rejected_reason)
        SELECT message_id, tokens_saved, savings_usd, created_at, reported_at,
               {rejected_at}, {rejected_reason}
        FROM savings_report_queue;
        DROP TABLE savings_report_queue;
        ALTER TABLE savings_report_queue_v2 RENAME TO savings_report_queue;
        CREATE INDEX IF NOT EXISTS idx_savings_report_queue_unreported
            ON savings_report_queue(created_at ASC, message_id ASC)
            WHERE reported_at IS NULL;"
    ))?;
    Ok(())
}

pub(super) fn apply_savings_dollars_migration(connection: &Connection) -> StorageResult<()> {
    if !column_exists(connection, "savings_report_queue", "kind")? {
        rebuild_report_queue(connection)?;
    }
    add_column(
        connection,
        "savings_report_queue",
        "rejected_at",
        "TEXT NULL",
    )?;
    add_column(
        connection,
        "savings_report_queue",
        "rejected_reason",
        "TEXT NULL",
    )?;
    // The model that the transcript shows, and how the user pays for it.
    add_column(connection, "agent_sessions", "observed_model", "TEXT NULL")?;
    add_column(
        connection,
        "agent_sessions",
        "billing",
        "TEXT NULL CHECK (billing IN ('api', 'plan', 'unknown') OR billing IS NULL)",
    )?;
    for (column, definition) in [
        ("model", "TEXT NULL"),
        ("usd_method", "TEXT NULL"),
        ("price_version", "TEXT NULL"),
        ("prompt_rate_usd_per_m", "REAL NULL"),
        ("source_kind", "TEXT NULL"),
        ("cache_write_1h_tokens", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        add_column(connection, "agent_session_turns", column, definition)?;
    }
    for table in ["savings_snapshots", "archived_savings_snapshots"] {
        add_column(connection, table, "usd_method", "TEXT NULL")?;
        add_column(connection, table, "price_version", "TEXT NULL")?;
    }
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_SAVINGS_DOLLARS, now_string()],
    )?;
    Ok(())
}

fn add_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> StorageResult<()> {
    if !column_exists(connection, table, column)? {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

/// The billing of a session lane: `plan` for a flat subscription, `api`
/// for a per-token bill.
pub(crate) fn lane_billing(lane: &str) -> &'static str {
    match lane {
        "claude_subscription" | "codex_chatgpt" | "kimi_subscription" | "zai_glm" => "plan",
        "claude_api_key" | "grok_api_key" | "kimi_api_key" | "custom" => "api",
        _ => "unknown",
    }
}

impl Storage {
    /// Queues a report event with the dollar fields of ADR 0015. A context
    /// event needs tokens; a routing event can have none.
    pub(crate) fn enqueue_priced_savings_report(
        &self,
        event_id: &str,
        tokens_saved: i64,
        dollars: &ReportDollars,
    ) -> StorageResult<()> {
        if tokens_saved < 0 || (tokens_saved == 0 && dollars.kind != "routing") {
            return Ok(());
        }
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let parts = report_event_parts(event_id, tokens_saved);
        if parts.len() > 1 {
            delete_unreported_report_parts(&transaction, event_id)?;
        }
        for part in parts {
            let net_usd = dollars.net_usd * part.share;
            transaction.execute(
                "INSERT INTO savings_report_queue
                    (message_id, tokens_saved, savings_usd, created_at, reported_at,
                     kind, net_usd, method, price_version)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8)
                 ON CONFLICT(message_id) DO UPDATE SET
                    tokens_saved = excluded.tokens_saved,
                    savings_usd = excluded.savings_usd,
                    kind = excluded.kind,
                    net_usd = excluded.net_usd,
                    method = excluded.method,
                    price_version = excluded.price_version
                 WHERE savings_report_queue.reported_at IS NULL",
                params![
                    part.id,
                    part.tokens_saved,
                    net_usd.max(0.0),
                    now_string(),
                    dollars.kind,
                    net_usd,
                    dollars.method,
                    dollars.price_version
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn set_agent_session_billing(
        &self,
        session_id: &str,
        billing: &str,
    ) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE agent_sessions SET billing = ?1 WHERE id = ?2",
                params![billing, session_id],
            )?;
        Ok(())
    }

    pub(crate) fn set_agent_session_observed_model(
        &self,
        session_id: &str,
        model: &str,
    ) -> StorageResult<()> {
        self.connection
            .lock()
            .expect("storage lock poisoned")
            .execute(
                "UPDATE agent_sessions SET observed_model = ?1
                 WHERE id = ?2 AND observed_model IS NOT ?1",
                params![model, session_id],
            )?;
        Ok(())
    }

    /// Marks events that the server refused as dead letters. The reason is
    /// the server error text, for diagnostics.
    pub(crate) fn mark_savings_rejected(&self, rejected: &[(String, String)]) -> StorageResult<()> {
        if rejected.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction()?;
        let rejected_at = now_string();
        for (event_id, reason) in rejected {
            transaction.execute(
                "UPDATE savings_report_queue
                 SET rejected_at = ?1, rejected_reason = ?2
                 WHERE message_id = ?3 AND reported_at IS NULL",
                params![&rejected_at, reason, event_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::new_uuid_v4;
    use super::*;
    use std::path::PathBuf;

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-dollars-{name}-{}.sqlite3", new_uuid_v4()))
    }

    #[test]
    fn a_dead_letter_leaves_the_queue_and_the_pending_totals() {
        let path = test_db_path("dead-letter");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .enqueue_savings_report("bad", 40, 0.40)
            .expect("savings should enqueue");
        storage
            .enqueue_savings_report("good", 10, 0.10)
            .expect("savings should enqueue");

        storage
            .mark_savings_rejected(&[("bad".to_string(), "invalid savings payload".to_string())])
            .expect("dead letter should save");

        let queued = storage.unreported_savings(200).expect("queue should load");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, "good");
        let totals = storage.total_savings().expect("totals should load");
        assert_eq!(totals.pending_tokens_saved, 10);

        let connection = storage.connection.lock().expect("storage lock should open");
        let reason: Option<String> = connection
            .query_row(
                "SELECT rejected_reason FROM savings_report_queue WHERE message_id = 'bad'",
                [],
                |row| row.get(0),
            )
            .expect("dead letter should load");
        assert_eq!(reason.as_deref(), Some("invalid savings payload"));
        drop(connection);
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_migration_adds_the_dollar_columns() {
        let connection = Connection::open_in_memory().expect("memory database should open");
        super::super::run_migrations(&connection).expect("migrations should run");
        for (table, column) in [
            ("agent_sessions", "observed_model"),
            ("agent_sessions", "billing"),
            ("agent_session_turns", "model"),
            ("agent_session_turns", "usd_method"),
            ("agent_session_turns", "price_version"),
            ("agent_session_turns", "prompt_rate_usd_per_m"),
            ("agent_session_turns", "source_kind"),
            ("agent_session_turns", "cache_write_1h_tokens"),
            ("savings_snapshots", "usd_method"),
            ("archived_savings_snapshots", "price_version"),
        ] {
            assert!(
                column_exists(&connection, table, column).unwrap(),
                "{table}.{column}"
            );
        }
    }

    #[test]
    fn plan_lanes_bill_as_plans() {
        assert_eq!(lane_billing("claude_subscription"), "plan");
        assert_eq!(lane_billing("codex_chatgpt"), "plan");
        assert_eq!(lane_billing("zai_glm"), "plan");
        assert_eq!(lane_billing("claude_api_key"), "api");
        assert_eq!(lane_billing("something_new"), "unknown");
    }

    #[test]
    fn the_queue_rebuild_keeps_every_row_and_allows_routing_events() {
        let connection = Connection::open_in_memory().expect("memory database should open");
        super::super::run_migrations(&connection).expect("migrations should run");
        // The queue as 0025 and 0047 made it, with one reported row.
        connection
            .execute_batch(
                "DROP TABLE savings_report_queue;
                 CREATE TABLE savings_report_queue (
                    message_id TEXT PRIMARY KEY,
                    tokens_saved INTEGER NOT NULL CHECK (tokens_saved > 0),
                    savings_usd REAL,
                    created_at TEXT NOT NULL,
                    reported_at TEXT
                 );
                 INSERT INTO savings_report_queue VALUES ('a', 10, 0.1, '2026-10-01', NULL);
                 INSERT INTO savings_report_queue VALUES ('b', 20, 0.2, '2026-10-02', '2026-10-03');",
            )
            .expect("old queue should create");
        apply_savings_dollars_migration(&connection).expect("rebuild should run");
        let rows: Vec<(String, i64, Option<String>, String)> = connection
            .prepare(
                "SELECT message_id, tokens_saved, reported_at, kind
                 FROM savings_report_queue ORDER BY message_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                ("a".to_string(), 10, None, "context".to_string()),
                (
                    "b".to_string(),
                    20,
                    Some("2026-10-03".to_string()),
                    "context".to_string()
                ),
            ]
        );
        connection
            .execute(
                "INSERT INTO savings_report_queue (message_id, tokens_saved, created_at, kind)
                 VALUES ('routing:1', 0, '2026-10-04', 'routing')",
                [],
            )
            .expect("a routing event with no tokens should queue");
    }

    #[test]
    fn priced_events_carry_their_dollar_fields() {
        let path = test_db_path("priced-events");
        let storage = Storage::open(&path).expect("storage should open");
        storage
            .enqueue_savings_report("legacy", 10, 0.10)
            .expect("legacy event should queue");
        storage
            .enqueue_priced_savings_report(
                "v2",
                20,
                &ReportDollars {
                    kind: "context".to_string(),
                    net_usd: 0.05,
                    method: "effective_rate".to_string(),
                    price_version: "2026-10-15".to_string(),
                },
            )
            .expect("priced event should queue");
        let events = storage.unreported_savings(200).expect("queue should load");
        assert!(events[0].created_at.is_some(), "the queue time goes along");
        let legacy = serde_json::to_value(super::super::SavingsReportEvent {
            created_at: None,
            ..events[0].clone()
        })
        .unwrap();
        assert_eq!(
            legacy,
            serde_json::json!({"id": "legacy", "tokens_saved": 10, "savings_usd": 0.10})
        );
        let v2 = serde_json::to_value(&events[1]).unwrap();
        assert_eq!(v2["kind"], "context");
        assert_eq!(v2["net_usd"], 0.05);
        assert_eq!(v2["method"], "effective_rate");
        assert_eq!(v2["price_version"], "2026-10-15");
        let totals = storage.total_savings().expect("totals should load");
        assert!((totals.pending_dollars_usd - 0.05).abs() < 1e-12);
        drop(storage);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_migration_is_idempotent() {
        let connection = Connection::open_in_memory().expect("memory database should open");
        super::super::run_migrations(&connection).expect("migrations should run");
        apply_savings_dollars_migration(&connection).expect("a second run should pass");
        assert!(column_exists(&connection, "savings_report_queue", "rejected_at").unwrap());
    }
}
