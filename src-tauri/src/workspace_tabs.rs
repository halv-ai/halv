//! Workspace tabs (ADR 0010): the commands of the frontend tab store.
//!
//! The frontend sends one batch of tab operations per user intent through a
//! serial queue. Rust applies one batch in one SQLite transaction and returns
//! the new snapshot. No command takes a session list or a layout, and no
//! command replaces all tabs. The SQL lives in `storage::workspace_tabs`.

use crate::storage::Storage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;

/// One row of `workspace_tabs`. Times are unix milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TabRow {
    /// `conversation:<conversationId>` or `shell:<uuid>`.
    pub key: String,
    pub conversation_id: Option<String>,
    pub shell_workspace_id: Option<String>,
    /// Shell tabs only.
    pub custom_title: Option<String>,
    /// Shell tabs only: "Terminal N", fixed at creation.
    pub default_title: Option<String>,
    pub position: i64,
    pub created_at: i64,
    pub activated_at: Option<i64>,
}

/// One tab operation. The tag and the fields match the TS `TabsOp` union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TabsOp {
    InsertTab {
        row: TabRow,
    },
    DeleteTab {
        key: String,
    },
    TouchTab {
        key: String,
        activated_at: i64,
    },
    SetPositions {
        positions: Vec<(String, i64)>,
    },
    SetTitle {
        key: String,
        title: Option<String>,
    },
    SetActive {
        workspace_id: String,
        key: Option<String>,
    },
}

/// All tab rows and active pointers. `rev` counts the committed batches of
/// this process, so the store can drop an older snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TabsSnapshot {
    pub rev: u64,
    pub rows: Vec<TabRow>,
    pub active_by_workspace: BTreeMap<String, Option<String>>,
    pub seeded: bool,
}

/// The process-lifetime revision. It starts at 0 on each launch.
static TABS_REV: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub async fn workspace_tabs_snapshot(
    storage: tauri::State<'_, Storage>,
) -> Result<TabsSnapshot, String> {
    storage
        .workspace_tabs_snapshot(&TABS_REV)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workspace_tabs_apply(
    storage: tauri::State<'_, Storage>,
    ops: Vec<TabsOp>,
) -> Result<TabsSnapshot, String> {
    storage
        .workspace_tabs_apply(&ops, &TABS_REV)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn workspace_tabs_seed(
    storage: tauri::State<'_, Storage>,
    rows: Vec<TabRow>,
    active: BTreeMap<String, Option<String>>,
) -> Result<TabsSnapshot, String> {
    storage
        .workspace_tabs_seed(&rows, &active, &TABS_REV)
        .map_err(|error| error.to_string())
}

/// The startup GC. It runs once after the migrations, before the window
/// loads. A failure keeps the rows and does not stop the start.
pub fn collect_garbage(storage: &Storage) {
    if let Err(error) = storage.workspace_tabs_gc() {
        eprintln!("workspace tabs: startup GC failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tabs_op_json_matches_the_ts_union() {
        let insert: TabsOp = serde_json::from_value(json!({
            "type": "insertTab",
            "row": {
                "key": "shell:0f8fad5b-d9cb-469f-a165-70867728950e",
                "conversationId": null,
                "shellWorkspaceId": "primary-1",
                "customTitle": null,
                "defaultTitle": "Terminal 1",
                "position": 2,
                "createdAt": 10,
                "activatedAt": null
            }
        }))
        .expect("insertTab should parse");
        assert!(matches!(insert, TabsOp::InsertTab { ref row } if row.position == 2));

        let positions: TabsOp = serde_json::from_value(json!({
            "type": "setPositions",
            "positions": [["conversation:a", 0], ["conversation:b", 1]]
        }))
        .expect("setPositions should parse");
        assert_eq!(
            positions,
            TabsOp::SetPositions {
                positions: vec![("conversation:a".into(), 0), ("conversation:b".into(), 1)],
            }
        );

        let touch = TabsOp::TouchTab {
            key: "conversation:a".into(),
            activated_at: 5,
        };
        assert_eq!(
            serde_json::to_value(&touch).expect("touchTab should serialize"),
            json!({ "type": "touchTab", "key": "conversation:a", "activatedAt": 5 })
        );
        let active = TabsOp::SetActive {
            workspace_id: "primary-1".into(),
            key: None,
        };
        assert_eq!(
            serde_json::to_value(&active).expect("setActive should serialize"),
            json!({ "type": "setActive", "workspaceId": "primary-1", "key": null })
        );
        assert!(serde_json::from_value::<TabsOp>(json!({ "type": "replaceAll" })).is_err());
    }

    #[test]
    fn snapshot_json_uses_camel_case() {
        let snapshot = TabsSnapshot {
            rev: 3,
            rows: Vec::new(),
            active_by_workspace: BTreeMap::from([("primary-1".to_string(), None)]),
            seeded: true,
        };
        assert_eq!(
            serde_json::to_value(&snapshot).expect("snapshot should serialize"),
            json!({ "rev": 3, "rows": [], "activeByWorkspace": { "primary-1": null }, "seeded": true })
        );
    }
}
