use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) const MAX_PERSISTED_SCROLLBACK_BYTES: usize = 256 * 1024;
const TERMINAL_STATE_FILE_NAME: &str = "terminal-state-v1.json";
const TERMINAL_STATE_MAX_AGE_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalPaneStateInput {
    #[serde(default)]
    pub pane_id: String,
    pub session_id: String,
    pub project_id: Option<i64>,
    pub conversation_id: Option<String>,
    pub project_name: String,
    pub cwd: String,
    pub execution_profile: String,
    pub agent_kind: Option<String>,
    pub lane_kind: Option<String>,
    pub credential_profile_id: Option<String>,
    pub custom_title: Option<String>,
    #[serde(default)]
    pub default_title: Option<String>,
    #[serde(default)]
    pub generated_title: Option<String>,
    #[serde(default)]
    pub ai_vault_title: Option<Value>,
    #[serde(default)]
    pub generation: Option<u64>,
    pub cli_session_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalPaneState {
    #[serde(default)]
    pub pane_id: String,
    pub session_id: String,
    pub project_id: Option<i64>,
    pub conversation_id: Option<String>,
    pub project_name: String,
    pub cwd: String,
    pub execution_profile: String,
    pub agent_kind: Option<String>,
    pub lane_kind: Option<String>,
    pub credential_profile_id: Option<String>,
    pub custom_title: Option<String>,
    #[serde(default)]
    pub default_title: Option<String>,
    #[serde(default)]
    pub generated_title: Option<String>,
    #[serde(default)]
    pub ai_vault_title: Option<Value>,
    #[serde(default)]
    pub generation: Option<u64>,
    pub cli_session_id: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TerminalPanelState {
    pub layout: Value,
    pub panes: Vec<TerminalPaneState>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScopedTerminalPanelState {
    pub scope: String,
    pub panel: TerminalPanelState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalStateFile {
    version: u8,
    panels: HashMap<String, TerminalPanelState>,
}

impl Default for TerminalStateFile {
    fn default() -> Self {
        Self {
            version: 1,
            panels: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct TerminalPersistence {
    path: Option<PathBuf>,
    state: TerminalStateFile,
}

impl TerminalPersistence {
    pub(crate) fn open(app_data_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(app_data_dir)?;
        let path = app_data_dir.join(TERMINAL_STATE_FILE_NAME);
        // The scrollback can hold secrets. An earlier release created the
        // file with the default mode.
        crate::app_data::restrict_file(&path);
        let mut state = match fs::read_to_string(&path) {
            Ok(serialized) => serde_json::from_str::<TerminalStateFile>(&serialized)
                .ok()
                .filter(|state| state.version == 1)
                .unwrap_or_default(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => TerminalStateFile::default(),
            Err(error) => return Err(error),
        };
        let migrated = migrate_terminal_pane_ids(&mut state);
        let pruned = prune_expired_panels(&mut state, Utc::now());
        let persistence = Self {
            path: Some(path),
            state,
        };
        if migrated || pruned {
            persistence.persist()?;
        }
        Ok(persistence)
    }

    pub(crate) fn panel(&self, scope: &str) -> Option<TerminalPanelState> {
        self.state.panels.get(scope).cloned()
    }

    pub(crate) fn panels(&self) -> Vec<ScopedTerminalPanelState> {
        let mut panels = self
            .state
            .panels
            .iter()
            .map(|(scope, panel)| ScopedTerminalPanelState {
                scope: scope.clone(),
                panel: panel.clone(),
            })
            .collect::<Vec<_>>();
        panels.sort_by(|left, right| right.panel.updated_at.cmp(&left.panel.updated_at));
        panels
    }

    pub(crate) fn pane(&self, scope: &str, session_id: &str) -> Option<TerminalPaneState> {
        self.state
            .panels
            .get(scope)?
            .panes
            .iter()
            .find(|pane| pane.session_id == session_id)
            .cloned()
    }

    pub(crate) fn save_panel(
        &mut self,
        scope: String,
        layout: Value,
        panes: Vec<TerminalPaneStateInput>,
        live_scrollback: &HashMap<String, String>,
    ) -> io::Result<()> {
        let existing_scrollback = self
            .state
            .panels
            .values()
            .flat_map(|panel| panel.panes.iter())
            .map(|pane| (pane.session_id.clone(), pane.scrollback.clone()))
            .collect::<HashMap<_, _>>();
        let now = Utc::now().to_rfc3339();
        let saved_session_ids = panes
            .iter()
            .map(|pane| pane.session_id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.state.panels.retain(|candidate_scope, panel| {
            if candidate_scope != &scope {
                panel
                    .panes
                    .retain(|pane| !saved_session_ids.contains(&pane.session_id));
            }
            !panel.panes.is_empty()
        });
        let panes = panes
            .into_iter()
            .filter(|pane| !pane.session_id.trim().is_empty())
            .map(|pane| {
                let pane_id = if pane.pane_id.trim().is_empty() {
                    pane.session_id.clone()
                } else {
                    pane.pane_id
                };
                TerminalPaneState {
                    pane_id,
                    scrollback: bounded_scrollback(
                        live_scrollback
                            .get(&pane.session_id)
                            .or_else(|| existing_scrollback.get(&pane.session_id))
                            .map(String::as_str)
                            .unwrap_or_default(),
                    ),
                    updated_at: now.clone(),
                    session_id: pane.session_id,
                    project_id: pane.project_id,
                    conversation_id: pane.conversation_id,
                    project_name: pane.project_name,
                    cwd: pane.cwd,
                    execution_profile: pane.execution_profile,
                    agent_kind: pane.agent_kind,
                    lane_kind: pane.lane_kind,
                    credential_profile_id: pane.credential_profile_id,
                    custom_title: pane.custom_title,
                    default_title: pane.default_title,
                    generated_title: pane.generated_title,
                    ai_vault_title: pane.ai_vault_title,
                    generation: pane.generation,
                    cli_session_id: pane.cli_session_id,
                    cols: pane.cols,
                    rows: pane.rows,
                }
            })
            .collect::<Vec<_>>();

        if panes.is_empty() {
            self.state.panels.remove(&scope);
        } else {
            self.state.panels.insert(
                scope,
                TerminalPanelState {
                    layout,
                    panes,
                    updated_at: now,
                },
            );
        }
        self.persist()
    }

    // Stages the scrollback in memory only. Terminal output arrives in many
    // small chunks; writing the state file for each chunk serializes every
    // reader thread on one mutex plus disk IO. A background flush persists
    // the staged state, and exit paths flush immediately.
    pub(crate) fn stage_scrollback(&mut self, session_id: &str, scrollback: &str) -> bool {
        let now = Utc::now().to_rfc3339();
        let scrollback = bounded_scrollback(scrollback);
        let mut changed = false;
        for panel in self.state.panels.values_mut() {
            if let Some(pane) = panel
                .panes
                .iter_mut()
                .find(|pane| pane.session_id == session_id)
            {
                pane.scrollback.clone_from(&scrollback);
                pane.updated_at.clone_from(&now);
                panel.updated_at.clone_from(&now);
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn update_title(&mut self, session_id: &str, title: Option<&str>) -> io::Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut changed = false;
        for panel in self.state.panels.values_mut() {
            if let Some(pane) = panel
                .panes
                .iter_mut()
                .find(|pane| pane.session_id == session_id)
            {
                pane.custom_title = title.map(ToString::to_string);
                pane.updated_at.clone_from(&now);
                panel.updated_at.clone_from(&now);
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    pub(crate) fn remove_session(&mut self, session_id: &str) -> io::Result<()> {
        let mut changed = false;
        self.state.panels.retain(|_, panel| {
            let previous_len = panel.panes.len();
            panel.panes.retain(|pane| pane.session_id != session_id);
            changed |= panel.panes.len() != previous_len;
            !panel.panes.is_empty()
        });
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    pub(crate) fn flush(&self) -> io::Result<()> {
        self.persist()
    }

    fn persist(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let serialized = serde_json::to_vec(&self.state).map_err(io::Error::other)?;
        crate::app_data::private_file_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?
            .write_all(&serialized)
    }
}

fn prune_expired_panels(state: &mut TerminalStateFile, now: DateTime<Utc>) -> bool {
    let previous_len = state.panels.len();
    let cutoff = now - Duration::days(TERMINAL_STATE_MAX_AGE_DAYS);
    state.panels.retain(|_, panel| {
        DateTime::parse_from_rfc3339(&panel.updated_at)
            .map(|updated_at| updated_at.with_timezone(&Utc) >= cutoff)
            .unwrap_or(false)
    });
    state.panels.len() != previous_len
}

fn migrate_terminal_pane_ids(state: &mut TerminalStateFile) -> bool {
    let mut migrated = false;
    for panel in state.panels.values_mut() {
        for pane in &mut panel.panes {
            if pane.pane_id.trim().is_empty() {
                pane.pane_id.clone_from(&pane.session_id);
                migrated = true;
            }
        }
    }
    migrated
}

fn bounded_scrollback(scrollback: &str) -> String {
    if scrollback.len() <= MAX_PERSISTED_SCROLLBACK_BYTES {
        return scrollback.to_string();
    }
    let mut start = scrollback.len() - MAX_PERSISTED_SCROLLBACK_BYTES;
    while !scrollback.is_char_boundary(start) {
        start += 1;
    }
    scrollback[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(session_id: &str) -> TerminalPaneStateInput {
        TerminalPaneStateInput {
            pane_id: session_id.to_string(),
            session_id: session_id.to_string(),
            project_id: Some(7),
            conversation_id: None,
            project_name: "Halv".to_string(),
            cwd: "/tmp/halv".to_string(),
            execution_profile: "full_access".to_string(),
            agent_kind: Some("claude".to_string()),
            lane_kind: Some("claudeSubscription".to_string()),
            credential_profile_id: None,
            custom_title: Some("Release checks".to_string()),
            default_title: Some("Claude".to_string()),
            generated_title: Some("Ship the release".to_string()),
            ai_vault_title: None,
            generation: Some(2),
            cli_session_id: Some("claude-session".to_string()),
            cols: 100,
            rows: 24,
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "halv-terminal-state-{label}-{:032x}",
            rand::random::<u128>()
        ))
    }

    #[test]
    fn state_round_trip_keeps_layout_metadata_and_scrollback() {
        let root = temp_dir("round-trip");
        let layout = serde_json::json!({
            "version": 1,
            "activeTabId": "terminal-1",
            "activeLeafId": "terminal-1",
            "tabs": [{ "tabId": "terminal-1", "root": { "type": "leaf", "leafId": "terminal-1" } }]
        });
        let mut scrollback = HashMap::new();
        scrollback.insert("terminal-1".to_string(), "saved output".to_string());

        let mut persistence = TerminalPersistence::open(&root).expect("state should open");
        persistence
            .save_panel(
                "project:7".to_string(),
                layout.clone(),
                vec![pane("terminal-1")],
                &scrollback,
            )
            .expect("state should save");
        drop(persistence);

        let restored = TerminalPersistence::open(&root)
            .expect("state should reopen")
            .panel("project:7")
            .expect("panel should restore");
        assert_eq!(restored.layout, layout);
        assert_eq!(restored.panes[0].scrollback, "saved output");
        assert_eq!(
            restored.panes[0].cli_session_id.as_deref(),
            Some("claude-session")
        );
        assert_eq!(restored.panes[0].default_title.as_deref(), Some("Claude"));
        assert_eq!(
            restored.panes[0].generated_title.as_deref(),
            Some("Ship the release")
        );
        assert_eq!(restored.panes[0].generation, Some(2));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn state_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("modes");
        let path = root.join(TERMINAL_STATE_FILE_NAME);
        let mode = || {
            fs::metadata(&path)
                .expect("state file should exist")
                .permissions()
                .mode()
                & 0o777
        };
        let mut persistence = TerminalPersistence::open(&root).expect("state should open");
        persistence
            .save_panel(
                "project:7".to_string(),
                serde_json::json!({ "version": 1 }),
                vec![pane("terminal-1")],
                &HashMap::new(),
            )
            .expect("state should save");
        assert_eq!(mode(), 0o600);

        // An earlier release left the file readable by others.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode should change");
        TerminalPersistence::open(&root).expect("state should reopen");
        assert_eq!(mode(), 0o600);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_continues_when_the_state_file_mode_cannot_change() {
        use crate::app_data::test_support::ImmutableFlag;
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("mode-rejected");
        fs::create_dir_all(&root).expect("state directory should be created");
        let path = root.join(TERMINAL_STATE_FILE_NAME);
        fs::write(&path, "{}").expect("state file should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode should change");
        {
            let _flag = ImmutableFlag::set(&path);
            TerminalPersistence::open(&root).expect("a rejected mode change should not stop open");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn state_open_migrates_a_missing_pane_id() {
        let root = temp_dir("missing-pane-id");
        fs::create_dir_all(&root).expect("test directory should exist");
        let updated_at = Utc::now().to_rfc3339();
        let state = serde_json::json!({
            "version": 1,
            "panels": {
                "project:7": {
                    "layout": {
                        "version": 1,
                        "activeTabId": "terminal-1",
                        "activeLeafId": "terminal-1",
                        "tabs": [{
                            "tabId": "terminal-1",
                            "root": { "type": "leaf", "leafId": "terminal-1" }
                        }]
                    },
                    "panes": [{
                        "sessionId": "terminal-1",
                        "projectId": 7,
                        "conversationId": null,
                        "projectName": "Halv",
                        "cwd": "/tmp/halv",
                        "executionProfile": "full_access",
                        "agentKind": null,
                        "laneKind": null,
                        "credentialProfileId": null,
                        "customTitle": null,
                        "cliSessionId": null,
                        "cols": 100,
                        "rows": 24,
                        "scrollback": "saved output",
                        "updatedAt": updated_at.clone()
                    }],
                    "updatedAt": updated_at
                }
            }
        });
        fs::write(
            root.join(TERMINAL_STATE_FILE_NAME),
            serde_json::to_vec(&state).expect("state should serialize"),
        )
        .expect("state should write");

        let persistence = TerminalPersistence::open(&root).expect("state should open");
        let restored = persistence
            .panel("project:7")
            .expect("panel should restore");
        assert_eq!(restored.panes[0].pane_id, "terminal-1");

        let saved: Value = serde_json::from_str(
            &fs::read_to_string(root.join(TERMINAL_STATE_FILE_NAME))
                .expect("state should remain on disk"),
        )
        .expect("saved state should parse");
        assert_eq!(
            saved["panels"]["project:7"]["panes"][0]["paneId"],
            "terminal-1"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn state_keeps_the_newest_bounded_utf8_scrollback() {
        let prefix = "🦀";
        let mut output = prefix.to_string();
        output.push_str(&"a".repeat(MAX_PERSISTED_SCROLLBACK_BYTES));
        output.push_str("tail");
        let bounded = bounded_scrollback(&output);

        assert!(bounded.len() <= MAX_PERSISTED_SCROLLBACK_BYTES);
        assert!(bounded.ends_with("tail"));
        assert!(!bounded.contains('�'));
    }

    #[test]
    fn staged_scrollback_reaches_disk_only_on_flush() {
        let root = temp_dir("stage");
        let mut persistence = TerminalPersistence::open(&root).expect("state should open");
        persistence
            .save_panel(
                "project:7".to_string(),
                serde_json::json!({ "version": 1 }),
                vec![pane("terminal-1")],
                &HashMap::new(),
            )
            .expect("state should save");

        assert!(persistence.stage_scrollback("terminal-1", "staged output"));
        assert!(!persistence.stage_scrollback("missing", "ignored"));
        let on_disk = TerminalPersistence::open(&root)
            .expect("state should reopen")
            .panel("project:7")
            .expect("panel should exist");
        assert_eq!(on_disk.panes[0].scrollback, "");

        persistence.flush().expect("state should flush");
        let flushed = TerminalPersistence::open(&root)
            .expect("state should reopen")
            .panel("project:7")
            .expect("panel should exist");
        assert_eq!(flushed.panes[0].scrollback, "staged output");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn old_panels_are_pruned_when_state_opens() {
        let root = temp_dir("prune");
        fs::create_dir_all(&root).expect("test directory should exist");
        let state = serde_json::json!({
            "version": 1,
            "panels": {
                "project:7": {
                    "layout": { "version": 1, "activeTabId": null, "activeLeafId": null, "tabs": [] },
                    "panes": [],
                    "updatedAt": "2020-01-01T00:00:00Z"
                }
            }
        });
        fs::write(
            root.join(TERMINAL_STATE_FILE_NAME),
            serde_json::to_vec(&state).expect("state should serialize"),
        )
        .expect("state should write");

        let persistence = TerminalPersistence::open(&root).expect("state should open");
        assert!(persistence.panel("project:7").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_session_removal_prunes_saved_panes() {
        let root = temp_dir("remove");
        let mut persistence = TerminalPersistence::open(&root).expect("state should open");
        persistence
            .save_panel(
                "project:7".to_string(),
                serde_json::json!({ "version": 1 }),
                vec![pane("terminal-1")],
                &HashMap::new(),
            )
            .expect("state should save");

        persistence
            .remove_session("terminal-1")
            .expect("session should be removed");
        assert!(persistence.panel("project:7").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saving_a_combined_panel_moves_panes_without_losing_scrollback() {
        let root = temp_dir("combine");
        let mut persistence = TerminalPersistence::open(&root).expect("state should open");
        let mut first_scrollback = HashMap::new();
        first_scrollback.insert("terminal-1".to_string(), "first output".to_string());
        persistence
            .save_panel(
                "agent:one".to_string(),
                serde_json::json!({ "version": 1 }),
                vec![pane("terminal-1")],
                &first_scrollback,
            )
            .expect("first panel should save");
        persistence
            .save_panel(
                "project:all".to_string(),
                serde_json::json!({ "version": 1 }),
                vec![pane("terminal-1")],
                &HashMap::new(),
            )
            .expect("combined panel should save");

        let panels = persistence.panels();
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].scope, "project:all");
        assert_eq!(panels[0].panel.panes[0].scrollback, "first output");
        let _ = fs::remove_dir_all(root);
    }
}
