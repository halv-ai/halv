//! SQL of the workspace tabs (ADR 0010, migration 0054_workspace_tabs).
//!
//! Rules:
//! - One batch of tab operations is one `BEGIN IMMEDIATE` transaction. Any
//!   error rolls back the whole batch.
//! - An insert uses the targetless `ON CONFLICT DO NOTHING`. It ignores a
//!   duplicate tab but keeps CHECK and foreign key errors. `INSERT OR IGNORE`
//!   would drop a malformed row in silence, so this module never uses it.
//! - `setActive` is a guarded no-op. A close carries the successor pointer,
//!   and a successor that another writer deleted must not roll back the close.
//! - The startup GC deletes only rows whose conversation is archived or has
//!   no project. It never joins `workspaces`, so a missing primary workspace
//!   row cannot delete a tab.

use super::{Storage, StorageError, StorageResult};
use crate::workspace_tabs::{TabRow, TabsOp, TabsSnapshot};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MIGRATION_WORKSPACE_TABS: &str = "0054_workspace_tabs";
pub(crate) const WORKSPACE_TABS_SQL: &str = include_str!("workspace_tabs.sql");

// The frontend disk model (src/workspaceTabs.ts) copies these statements.
pub(crate) const INSERT_TAB_SQL: &str = "INSERT INTO workspace_tabs (tab_key, conversation_id, shell_workspace_id, custom_title, default_title, position, created_at, activated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT DO NOTHING";
pub(crate) const DELETE_TAB_SQL: &str = "DELETE FROM workspace_tabs WHERE tab_key = ?1";
pub(crate) const TOUCH_TAB_SQL: &str =
    "UPDATE workspace_tabs SET activated_at = ?1 WHERE tab_key = ?2";
pub(crate) const SET_POSITION_SQL: &str =
    "UPDATE workspace_tabs SET position = ?1 WHERE tab_key = ?2";
pub(crate) const SET_TITLE_SQL: &str =
    "UPDATE workspace_tabs SET custom_title = ?1 WHERE tab_key = ?2 AND conversation_id IS NULL";
pub(crate) const SET_ACTIVE_SQL: &str = "INSERT INTO workspace_tab_state (workspace_id, active_tab_key, updated_at) SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM workspaces WHERE id = ?1) AND (?2 IS NULL OR EXISTS (SELECT 1 FROM workspace_tabs WHERE tab_key = ?2)) ON CONFLICT(workspace_id) DO UPDATE SET active_tab_key = excluded.active_tab_key, updated_at = excluded.updated_at";
pub(crate) const GC_SQL: &str = "DELETE FROM workspace_tabs WHERE conversation_id IN (SELECT id FROM conversations WHERE archived_at IS NOT NULL OR project_id IS NULL)";
const SEED_MARKER_SQL: &str =
    "INSERT INTO workspace_tab_seed (id, seeded_at) VALUES (1, ?1) ON CONFLICT DO NOTHING";
// A seed row whose conversation went away, or is archived or project-less
// now, is skipped. The seed must not fail because of one stale candidate.
const SEED_TAB_SQL: &str = "INSERT INTO workspace_tabs (tab_key, conversation_id, shell_workspace_id, custom_title, default_title, position, created_at, activated_at) SELECT ?1, ?2, NULL, NULL, NULL, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM conversations WHERE id = ?2 AND archived_at IS NULL AND project_id IS NOT NULL) ON CONFLICT DO NOTHING";

pub(super) fn apply_workspace_tabs_migration(connection: &Connection) -> StorageResult<()> {
    connection.execute_batch(WORKSPACE_TABS_SQL)?;
    connection.execute(
        "INSERT OR IGNORE INTO migrations (version, applied_at) VALUES (?1, ?2)",
        params![MIGRATION_WORKSPACE_TABS, super::now_string()],
    )?;
    Ok(())
}

/// A shell tab key is `shell:` plus a UUID in the 8-4-4-4-12 hex form.
fn is_shell_tab_key(key: &str) -> bool {
    let Some(id) = key.strip_prefix("shell:") else {
        return false;
    };
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, length)| {
            group.len() == length && group.chars().all(|c| c.is_ascii_hexdigit())
        })
}

fn validate_tab_key(key: &str) -> StorageResult<()> {
    if key.starts_with("conversation:") || is_shell_tab_key(key) {
        return Ok(());
    }
    Err(StorageError::Validation(format!(
        "`{key}` is not a tab key: use `conversation:<id>` or `shell:<uuid>`"
    )))
}

fn apply_op(connection: &Connection, op: &TabsOp, now: i64) -> StorageResult<()> {
    match op {
        TabsOp::InsertTab { row } => {
            validate_tab_key(&row.key)?;
            connection.execute(
                INSERT_TAB_SQL,
                params![
                    row.key,
                    row.conversation_id,
                    row.shell_workspace_id,
                    row.custom_title,
                    row.default_title,
                    row.position,
                    row.created_at,
                    row.activated_at,
                ],
            )?;
        }
        TabsOp::DeleteTab { key } => {
            connection.execute(DELETE_TAB_SQL, params![key])?;
        }
        TabsOp::TouchTab { key, activated_at } => {
            connection.execute(TOUCH_TAB_SQL, params![activated_at, key])?;
        }
        TabsOp::SetPositions { positions } => {
            for (key, position) in positions {
                connection.execute(SET_POSITION_SQL, params![position, key])?;
            }
        }
        TabsOp::SetTitle { key, title } => {
            connection.execute(SET_TITLE_SQL, params![title, key])?;
        }
        TabsOp::SetActive { workspace_id, key } => {
            connection.execute(SET_ACTIVE_SQL, params![workspace_id, key, now])?;
        }
    }
    Ok(())
}

fn read_snapshot(connection: &Connection, rev: u64) -> StorageResult<TabsSnapshot> {
    let mut statement = connection.prepare(
        "SELECT tab_key, conversation_id, shell_workspace_id, custom_title, default_title,
                position, created_at, activated_at
         FROM workspace_tabs ORDER BY tab_key",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(TabRow {
                key: row.get(0)?,
                conversation_id: row.get(1)?,
                shell_workspace_id: row.get(2)?,
                custom_title: row.get(3)?,
                default_title: row.get(4)?,
                position: row.get(5)?,
                created_at: row.get(6)?,
                activated_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement =
        connection.prepare("SELECT workspace_id, active_tab_key FROM workspace_tab_state")?;
    let active_by_workspace = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let seeded: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_tab_seed WHERE id = 1)",
        [],
        |row| row.get(0),
    )?;
    Ok(TabsSnapshot {
        rev,
        rows,
        active_by_workspace,
        seeded,
    })
}

impl Storage {
    /// All rows and active pointers with the current revision.
    pub(crate) fn workspace_tabs_snapshot(&self, rev: &AtomicU64) -> StorageResult<TabsSnapshot> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        read_snapshot(&connection, rev.load(Ordering::SeqCst))
    }

    /// Applies one batch in one transaction. The revision grows only when
    /// the batch commits.
    pub(crate) fn workspace_tabs_apply(
        &self,
        ops: &[TabsOp],
        rev: &AtomicU64,
    ) -> StorageResult<TabsSnapshot> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().timestamp_millis();
        for op in ops {
            // Dropping the transaction on an error rolls the batch back.
            apply_op(&transaction, op, now)?;
        }
        transaction.commit()?;
        let next = rev.fetch_add(1, Ordering::SeqCst) + 1;
        read_snapshot(&connection, next)
    }

    /// The one-time seed. The marker insert decides: when the marker already
    /// exists, nothing else is written, also when the user closed every tab.
    pub(crate) fn workspace_tabs_seed(
        &self,
        rows: &[TabRow],
        active: &BTreeMap<String, Option<String>>,
        rev: &AtomicU64,
    ) -> StorageResult<TabsSnapshot> {
        let mut connection = self.connection.lock().expect("storage lock poisoned");
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = Utc::now().timestamp_millis();
        let claimed = transaction.execute(SEED_MARKER_SQL, params![now])? == 1;
        if !claimed {
            transaction.commit()?;
            return read_snapshot(&connection, rev.load(Ordering::SeqCst));
        }
        for row in rows {
            let Some(conversation_id) = row.conversation_id.as_deref() else {
                return Err(StorageError::Validation(
                    "the seed opens conversation tabs only".to_string(),
                ));
            };
            if row.key != format!("conversation:{conversation_id}") {
                return Err(StorageError::Validation(format!(
                    "`{}` is not the tab key of conversation `{conversation_id}`",
                    row.key
                )));
            }
            transaction.execute(
                SEED_TAB_SQL,
                params![
                    row.key,
                    conversation_id,
                    row.position,
                    row.created_at,
                    row.activated_at
                ],
            )?;
        }
        for (workspace_id, key) in active {
            transaction.execute(SET_ACTIVE_SQL, params![workspace_id, key, now])?;
        }
        transaction.commit()?;
        let next = rev.fetch_add(1, Ordering::SeqCst) + 1;
        read_snapshot(&connection, next)
    }

    /// The workspace of a shell tab, or None when no shell tab has this key.
    /// `terminal_open` checks the owner of a new shell with it.
    pub(crate) fn workspace_tab_shell_workspace(&self, key: &str) -> StorageResult<Option<String>> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        let workspace = connection
            .query_row(
                "SELECT shell_workspace_id FROM workspace_tabs
                 WHERE tab_key = ?1 AND conversation_id IS NULL",
                params![key],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(workspace.flatten())
    }

    /// The startup GC: rows of archived and project-less conversations.
    pub(crate) fn workspace_tabs_gc(&self) -> StorageResult<usize> {
        let connection = self.connection.lock().expect("storage lock poisoned");
        Ok(connection.execute(GC_SQL, [])?)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{new_uuid_v4, NewWorkspace};
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    const SHELL_A: &str = "shell:0f8fad5b-d9cb-469f-a165-70867728950e";
    const SHELL_B: &str = "shell:7c9e6679-7425-40de-944b-e07fc1f90ae7";

    fn test_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("halv-tabs-{name}-{}.sqlite3", new_uuid_v4()))
    }

    /// One project with its primary workspace and one managed worktree,
    /// and conversations on the primary, on the worktree, a legacy one with
    /// no workspace, and one with no project.
    struct World {
        storage: Storage,
        path: PathBuf,
        project_id: i64,
        primary: String,
        worktree: String,
        on_primary: String,
        on_worktree: String,
        legacy: String,
        loose: String,
    }

    impl World {
        fn new(name: &str) -> Self {
            let path = test_db_path(name);
            let storage = Storage::open(&path).expect("storage should open");
            let project = storage
                .upsert_project("repo", &format!("/tmp/halv-tabs-{}", new_uuid_v4()))
                .expect("project should insert");
            let primary = format!("primary-{}", project.id);
            let worktree_root = format!("/tmp/halv-tabs-wt-{}", new_uuid_v4());
            let worktree = storage
                .insert_managed_workspace(NewWorkspace {
                    project_id: project.id,
                    name: "halv/tabs",
                    root_path: &worktree_root,
                    git_common_dir: None,
                    base_ref: Some("HEAD"),
                    base_commit: None,
                    branch: Some("halv/tabs"),
                    local_environment_config_path: None,
                })
                .expect("worktree should insert")
                .id;
            let on_primary = storage
                .set_conversation_goal(None, None, None, Some(project.id))
                .expect("primary conversation should insert")
                .id;
            let on_worktree = storage
                .set_conversation_goal_in_workspace(
                    None,
                    None,
                    None,
                    Some(project.id),
                    Some(&worktree),
                )
                .expect("worktree conversation should insert")
                .id;
            let legacy = storage
                .set_conversation_goal(None, None, None, Some(project.id))
                .expect("legacy conversation should insert")
                .id;
            storage
                .with_connection(|connection| {
                    connection.execute(
                        "UPDATE conversations SET workspace_id = NULL WHERE id = ?1",
                        params![legacy],
                    )
                })
                .expect("legacy conversation should lose its workspace");
            let loose = storage
                .set_conversation_goal(None, None, None, None)
                .expect("loose conversation should insert")
                .id;
            Self {
                storage,
                path,
                project_id: project.id,
                primary,
                worktree,
                on_primary,
                on_worktree,
                legacy,
                loose,
            }
        }

        fn apply(&self, ops: Vec<TabsOp>, rev: &AtomicU64) -> StorageResult<TabsSnapshot> {
            self.storage.workspace_tabs_apply(&ops, rev)
        }

        fn snapshot(&self) -> TabsSnapshot {
            self.storage
                .workspace_tabs_snapshot(&AtomicU64::new(0))
                .expect("snapshot should read")
        }

        fn keys(&self) -> Vec<String> {
            self.snapshot()
                .rows
                .into_iter()
                .map(|row| row.key)
                .collect()
        }

        fn active(&self, workspace_id: &str) -> Option<Option<String>> {
            self.snapshot()
                .active_by_workspace
                .get(workspace_id)
                .cloned()
        }

        fn execute(&self, sql: &str) -> rusqlite::Result<usize> {
            self.storage
                .with_connection(|connection| connection.execute(sql, []))
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    impl Storage {
        fn with_connection<T>(
            &self,
            body: impl FnOnce(&Connection) -> rusqlite::Result<T>,
        ) -> rusqlite::Result<T> {
            let connection = self.connection.lock().expect("storage lock poisoned");
            body(&connection)
        }
    }

    fn conversation_row(conversation_id: &str, position: i64) -> TabRow {
        TabRow {
            key: format!("conversation:{conversation_id}"),
            conversation_id: Some(conversation_id.to_string()),
            shell_workspace_id: None,
            custom_title: None,
            default_title: None,
            position,
            created_at: 1,
            activated_at: None,
        }
    }

    fn shell_row(key: &str, workspace_id: &str, position: i64) -> TabRow {
        TabRow {
            key: key.to_string(),
            conversation_id: None,
            shell_workspace_id: Some(workspace_id.to_string()),
            custom_title: None,
            default_title: Some("Terminal 1".to_string()),
            position,
            created_at: 1,
            activated_at: None,
        }
    }

    fn insert(row: TabRow) -> TabsOp {
        TabsOp::InsertTab { row }
    }

    fn set_active(workspace_id: &str, key: Option<&str>) -> TabsOp {
        TabsOp::SetActive {
            workspace_id: workspace_id.to_string(),
            key: key.map(str::to_string),
        }
    }

    fn assert_constraint(result: StorageResult<TabsSnapshot>, needle: &str) {
        let error = result.expect_err("the batch should fail").to_string();
        assert!(
            error.contains(needle),
            "`{error}` should name the `{needle}` constraint"
        );
    }

    #[test]
    fn migration_is_idempotent_and_recorded_once() {
        let world = World::new("migration");
        drop(Storage::open(&world.path).expect("a second open should not fail"));
        let reopened = Storage::open(&world.path).expect("a third open should not fail");
        let (versions, tables) = reopened
            .with_connection(|connection| {
                let versions: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM migrations WHERE version = ?1",
                    params![MIGRATION_WORKSPACE_TABS],
                    |row| row.get(0),
                )?;
                let tables: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'
                     AND name IN ('workspace_tabs', 'workspace_tab_state', 'workspace_tab_seed')",
                    [],
                    |row| row.get(0),
                )?;
                Ok((versions, tables))
            })
            .expect("migration state should read");
        assert_eq!(versions, 1);
        assert_eq!(tables, 3);
    }

    #[test]
    fn a_lower_numbered_migration_that_lands_later_still_applies() {
        // Each migration checks its own version row. A database that already
        // has 0054 still gets a migration with a lower number that a later
        // build adds (the 0041 and 0042 pair already landed in that order).
        let world = World::new("out-of-order");
        world
            .storage
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM migrations WHERE version = '0052_task_hierarchies'",
                    [],
                )?;
                connection.execute_batch("DROP TABLE task_hierarchies")
            })
            .expect("0052 should roll back");
        drop(Storage::open(&world.path).expect("reopen should migrate"));
        let reopened = Storage::open(&world.path).expect("reopen should not fail");
        let restored: i64 = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM migrations
                     WHERE version IN ('0052_task_hierarchies', '0054_workspace_tabs')",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("versions should read");
        assert_eq!(restored, 2);
    }

    #[test]
    fn one_row_per_key_and_one_tab_per_conversation() {
        let world = World::new("unique");
        let rev = AtomicU64::new(0);
        let row = conversation_row(&world.on_primary, 0);
        world
            .apply(vec![insert(row.clone()), insert(row.clone())], &rev)
            .expect("a duplicate insert is a no-op");
        world
            .apply(vec![insert(row.clone())], &rev)
            .expect("a repeated open is a no-op");
        assert_eq!(world.keys(), vec![row.key.clone()]);

        let duplicate = format!(
            "INSERT INTO workspace_tabs (tab_key, conversation_id, created_at)
             VALUES ('conversation:{0}', '{0}', 2)",
            world.on_primary
        );
        let error = world
            .execute(&duplicate)
            .expect_err("a plain insert conflicts");
        assert!(error.to_string().contains("UNIQUE"), "{error}");
        let second_key = format!(
            "INSERT INTO workspace_tabs (tab_key, conversation_id, created_at)
             VALUES ('conversation:{0}#2', '{0}', 2)",
            world.on_primary
        );
        assert!(
            world.execute(&second_key).is_err(),
            "a second key for one conversation"
        );
    }

    #[test]
    fn rows_are_shape_checked_and_owners_must_exist() {
        let world = World::new("shape");
        let rev = AtomicU64::new(0);
        let mut with_workspace = conversation_row(&world.on_primary, 0);
        with_workspace.shell_workspace_id = Some(world.primary.clone());
        assert_constraint(world.apply(vec![insert(with_workspace)], &rev), "CHECK");

        let mut with_title = conversation_row(&world.on_primary, 0);
        with_title.custom_title = Some("x".into());
        assert_constraint(world.apply(vec![insert(with_title)], &rev), "CHECK");

        let mut wrong_key = conversation_row(&world.on_primary, 0);
        wrong_key.key = format!("conversation:{}", world.on_worktree);
        assert_constraint(world.apply(vec![insert(wrong_key)], &rev), "CHECK");

        let mut no_workspace = shell_row(SHELL_A, &world.primary, 0);
        no_workspace.shell_workspace_id = None;
        assert_constraint(world.apply(vec![insert(no_workspace)], &rev), "CHECK");

        let mut no_default_title = shell_row(SHELL_A, &world.primary, 0);
        no_default_title.default_title = None;
        assert_constraint(world.apply(vec![insert(no_default_title)], &rev), "CHECK");

        assert_constraint(
            world.apply(vec![insert(conversation_row("missing", 0))], &rev),
            "FOREIGN KEY",
        );
        assert_constraint(
            world.apply(vec![insert(shell_row(SHELL_A, "gone", 0))], &rev),
            "FOREIGN KEY",
        );
        for key in [
            "shell:not-a-uuid",
            "SHELL:0f8fad5b-d9cb-469f-a165-70867728950e",
            "workspace:x",
        ] {
            assert_constraint(
                world.apply(vec![insert(shell_row(key, &world.primary, 0))], &rev),
                "not a tab key",
            );
        }
        assert!(world.keys().is_empty());
        assert_eq!(
            rev.load(Ordering::SeqCst),
            0,
            "failed batches keep the revision"
        );
    }

    #[test]
    fn targetless_upsert_keeps_check_errors_that_insert_or_ignore_drops() {
        let world = World::new("upsert");
        let rev = AtomicU64::new(0);
        let mut malformed = conversation_row(&world.on_primary, 0);
        malformed.custom_title = Some("not allowed".into());
        assert_constraint(world.apply(vec![insert(malformed)], &rev), "CHECK");

        let ignored = world
            .execute(&format!(
                "INSERT OR IGNORE INTO workspace_tabs (tab_key, conversation_id, custom_title, created_at)
                 VALUES ('conversation:{0}', '{0}', 'not allowed', 1)",
                world.on_primary
            ))
            .expect("INSERT OR IGNORE hides the CHECK error");
        assert_eq!(ignored, 0, "the malformed row vanished in silence");
        assert!(world.keys().is_empty());
    }

    #[test]
    fn set_active_is_a_guarded_no_op() {
        let world = World::new("set-active");
        let rev = AtomicU64::new(0);
        let key = format!("conversation:{}", world.on_primary);
        world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    set_active(&world.primary, Some(&key)),
                ],
                &rev,
            )
            .expect("open should commit");
        assert_eq!(world.active(&world.primary), Some(Some(key.clone())));

        world
            .apply(
                vec![
                    set_active("gone", Some(&key)),
                    set_active(&world.primary, Some("conversation:missing")),
                ],
                &rev,
            )
            .expect("stale pointers are no-ops");
        assert_eq!(world.active(&world.primary), Some(Some(key)));
        assert_eq!(world.active("gone"), None);

        world
            .apply(vec![set_active(&world.primary, None)], &rev)
            .expect("a null pointer is valid");
        assert_eq!(world.active(&world.primary), Some(None));
    }

    #[test]
    fn a_batch_is_atomic_and_a_close_with_a_stale_successor_commits() {
        let world = World::new("atomic");
        let rev = AtomicU64::new(0);
        let first = format!("conversation:{}", world.on_primary);
        let second = format!("conversation:{}", world.legacy);
        assert_constraint(
            world.apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    TabsOp::TouchTab {
                        key: first.clone(),
                        activated_at: 5,
                    },
                    insert(conversation_row("missing", 1)),
                ],
                &rev,
            ),
            "FOREIGN KEY",
        );
        assert!(
            world.keys().is_empty(),
            "nothing from the failed intent stays"
        );

        let opened = world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    insert(conversation_row(&world.legacy, 1)),
                    TabsOp::SetPositions {
                        positions: vec![(first.clone(), 0), (second.clone(), 1)],
                    },
                    TabsOp::TouchTab {
                        key: second.clone(),
                        activated_at: 7,
                    },
                    set_active(&world.primary, Some(&second)),
                ],
                &rev,
            )
            .expect("open should commit");
        assert_eq!(opened.rev, 1);

        // Another writer removes the successor before the close lands.
        world
            .apply(vec![TabsOp::DeleteTab { key: first.clone() }], &rev)
            .expect("delete should commit");
        let closed = world
            .apply(
                vec![
                    TabsOp::DeleteTab { key: second },
                    set_active(&world.primary, Some(&first)),
                ],
                &rev,
            )
            .expect("the close must commit");
        assert!(closed.rows.is_empty());
        assert_eq!(closed.active_by_workspace.get(&world.primary), Some(&None));
        assert_eq!(closed.rev, 3);
    }

    #[test]
    fn titles_change_on_shell_tabs_only() {
        let world = World::new("titles");
        let rev = AtomicU64::new(0);
        let conversation = format!("conversation:{}", world.on_primary);
        world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    insert(shell_row(SHELL_A, &world.primary, 1)),
                    TabsOp::SetTitle {
                        key: SHELL_A.into(),
                        title: Some("dev server".into()),
                    },
                    TabsOp::SetTitle {
                        key: conversation.clone(),
                        title: Some("ignored".into()),
                    },
                ],
                &rev,
            )
            .expect("titles should commit");
        let snapshot = world.snapshot();
        let title_of = |key: &str| {
            snapshot
                .rows
                .iter()
                .find(|row| row.key == key)
                .expect("row should exist")
                .custom_title
                .clone()
        };
        assert_eq!(title_of(SHELL_A), Some("dev server".to_string()));
        assert_eq!(title_of(&conversation), None);
    }

    #[test]
    fn a_shell_tab_names_its_workspace_for_the_terminal_owner_check() {
        let world = World::new("shell-owner");
        let rev = AtomicU64::new(0);
        let conversation = format!("conversation:{}", world.on_primary);
        world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    insert(shell_row(SHELL_A, &world.worktree, 1)),
                ],
                &rev,
            )
            .expect("tabs should commit");
        let lookup = |key: &str| {
            world
                .storage
                .workspace_tab_shell_workspace(key)
                .expect("the lookup should run")
        };
        assert_eq!(lookup(SHELL_A), Some(world.worktree.clone()));
        assert_eq!(lookup(SHELL_B), None, "an unknown shell tab");
        assert_eq!(
            lookup(&conversation),
            None,
            "a conversation tab owns no shell"
        );
    }

    #[test]
    fn the_seed_runs_once_also_when_two_starts_race() {
        let world = World::new("seed");
        let rev = AtomicU64::new(0);
        let key = format!("conversation:{}", world.on_primary);
        let rows = vec![
            conversation_row(&world.on_primary, 0),
            conversation_row(&world.on_worktree, 0),
            conversation_row(&world.loose, 0),
            conversation_row("deleted-meanwhile", 1),
        ];
        let active = BTreeMap::from([(world.primary.clone(), Some(key.clone()))]);
        let seeded = world
            .storage
            .workspace_tabs_seed(&rows, &active, &rev)
            .expect("seed should commit");
        assert!(seeded.seeded);
        assert_eq!(
            seeded.rows.len(),
            2,
            "stale and project-less candidates are skipped"
        );
        assert_eq!(
            seeded.active_by_workspace.get(&world.primary),
            Some(&Some(key))
        );

        world
            .apply(
                seeded
                    .rows
                    .iter()
                    .map(|row| TabsOp::DeleteTab {
                        key: row.key.clone(),
                    })
                    .collect(),
                &rev,
            )
            .expect("the user closes every tab");
        let again = world
            .storage
            .workspace_tabs_seed(&rows, &active, &rev)
            .expect("a second seed is a no-op");
        assert!(again.rows.is_empty(), "a later start never seeds again");

        let shell = world.storage.workspace_tabs_seed(
            &[shell_row(SHELL_B, &world.primary, 0)],
            &BTreeMap::new(),
            &rev,
        );
        assert!(shell.is_ok(), "the marker exists, so the seed reads only");

        // Two app starts race on one fresh database file.
        let race = World::new("seed-race");
        let row = conversation_row(&race.on_primary, 0);
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = race.path.clone();
                let row = row.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let storage = Storage::open(&path).expect("racer should open");
                    let rev = AtomicU64::new(0);
                    barrier.wait();
                    storage
                        .workspace_tabs_seed(&[row], &BTreeMap::new(), &rev)
                        .expect("racer should finish");
                    rev.load(Ordering::SeqCst)
                })
            })
            .collect();
        let claims: u64 = handles
            .into_iter()
            .map(|handle| handle.join().expect("racer should not panic"))
            .sum();
        assert_eq!(claims, 1, "exactly one start seeds");
        assert_eq!(race.keys().len(), 1);
    }

    #[test]
    fn the_seed_rejects_shell_rows() {
        let world = World::new("seed-shape");
        let rev = AtomicU64::new(0);
        let error = world
            .storage
            .workspace_tabs_seed(
                &[shell_row(SHELL_A, &world.primary, 0)],
                &BTreeMap::new(),
                &rev,
            )
            .expect_err("a shell seed row is invalid");
        assert!(error.to_string().contains("conversation tabs only"));
        assert!(
            !world.snapshot().seeded,
            "a failed seed rolls the marker back"
        );
    }

    #[test]
    fn the_gc_deletes_archived_and_project_less_rows_only() {
        let world = World::new("gc");
        let rev = AtomicU64::new(0);
        world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    insert(conversation_row(&world.on_worktree, 0)),
                    insert(conversation_row(&world.legacy, 1)),
                    insert(conversation_row(&world.loose, 0)),
                    insert(shell_row(SHELL_A, &world.worktree, 1)),
                ],
                &rev,
            )
            .expect("open should commit");
        world
            .storage
            .set_conversation_archived(&world.on_worktree, true)
            .expect("archive should commit");
        // A missing primary workspace row must not delete the legacy tab.
        world
            .storage
            .with_connection(|connection| {
                connection.execute(
                    "DELETE FROM workspaces WHERE id = ?1",
                    params![world.primary],
                )
            })
            .expect("the primary row should delete");
        assert_eq!(world.keys().len(), 5, "an archive only hides the row");

        let deleted = world.storage.workspace_tabs_gc().expect("gc should run");
        assert_eq!(deleted, 2);
        let mut kept = world.keys();
        kept.sort();
        let mut expected = vec![
            format!("conversation:{}", world.on_primary),
            format!("conversation:{}", world.legacy),
            SHELL_A.to_string(),
        ];
        expected.sort();
        assert_eq!(kept, expected);
    }

    #[test]
    fn existing_statements_move_hide_and_cascade_tabs() {
        let world = World::new("replay");
        let rev = AtomicU64::new(0);
        let primary_tab = format!("conversation:{}", world.on_primary);
        world
            .apply(
                vec![
                    insert(conversation_row(&world.on_primary, 0)),
                    insert(conversation_row(&world.legacy, 1)),
                    insert(shell_row(SHELL_A, &world.worktree, 0)),
                    insert(shell_row(SHELL_B, &world.primary, 2)),
                    set_active(&world.primary, Some(&primary_tab)),
                    set_active(&world.worktree, Some(SHELL_A)),
                ],
                &rev,
            )
            .expect("open should commit");

        // Handoff and mode changes write only the conversation row.
        world
            .storage
            .handoff_conversation_workspace(&world.on_primary, &world.worktree, &world.worktree)
            .expect("handoff should commit");
        world
            .storage
            .set_conversation_mode(&world.on_primary, "agent")
            .expect("mode should commit");
        let row = world
            .snapshot()
            .rows
            .into_iter()
            .find(|row| row.key == primary_tab)
            .expect("the tab survives a handoff");
        assert_eq!(row.shell_workspace_id, None, "the home stays derived");

        // A conversation delete cascades its tab and clears the pointer.
        world
            .storage
            .delete_conversation(&world.on_primary)
            .expect("delete should commit");
        assert!(!world.keys().contains(&primary_tab));
        assert_eq!(world.active(&world.primary), Some(None));

        // A worktree removal cascades its shell tabs and its state row.
        world
            .storage
            .delete_managed_workspace(&world.worktree)
            .expect("worktree removal should commit");
        assert!(!world.keys().contains(&SHELL_A.to_string()));
        assert_eq!(world.active(&world.worktree), None);

        // A project removal drops the primary shell tabs and hides the
        // conversation tabs. The next GC removes the hidden rows.
        world
            .storage
            .remove_project(world.project_id)
            .expect("project removal should commit");
        assert_eq!(world.keys(), vec![format!("conversation:{}", world.legacy)]);
        assert_eq!(world.storage.workspace_tabs_gc().expect("gc should run"), 1);
        assert!(world.keys().is_empty());
    }

    #[test]
    fn snapshots_carry_the_revision_of_the_last_commit() {
        let world = World::new("rev");
        let rev = AtomicU64::new(0);
        assert_eq!(
            world
                .storage
                .workspace_tabs_snapshot(&rev)
                .expect("read")
                .rev,
            0
        );
        let applied = world
            .apply(vec![insert(conversation_row(&world.on_primary, 0))], &rev)
            .expect("open should commit");
        assert_eq!(applied.rev, 1);
        assert!(world
            .apply(vec![insert(conversation_row("missing", 0))], &rev)
            .is_err());
        let read = world.storage.workspace_tabs_snapshot(&rev).expect("read");
        assert_eq!(read.rev, 1);
        assert_eq!(read.rows, applied.rows);
        assert!(!read.seeded);
    }
}
