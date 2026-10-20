//! The Claude config folder of each agent pane.
//!
//! Claude Code writes its presence file under the `CLAUDE_CONFIG_DIR` of the
//! process. A wallet account gives each launch its own folder, and the
//! active account can change while a pane runs. The launch records the
//! folder of each pane in a small file, so an adopted pane reads the same
//! folder after an app restart.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

const CONFIG_DIR_VARIABLE: &str = "CLAUDE_CONFIG_DIR";
const REGISTRY_FILE: &str = "config-dirs.json";
/// A record older than this has no live pane. The PTY supervisor keeps
/// panes for days at most.
const RECORD_LIFETIME_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// The folder that Claude Code uses with this launch environment.
pub(crate) fn claude_config_dir_for_launch(
    env: &[(OsString, OsString)],
    env_remove: &[OsString],
) -> Option<PathBuf> {
    let name = OsStr::new(CONFIG_DIR_VARIABLE);
    if let Some((_, value)) = env.iter().rev().find(|(candidate, _)| candidate == name) {
        if !value.is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    if env_remove.iter().any(|candidate| candidate == name) {
        return default_config_dir();
    }
    system_config_dir()
}

/// `CLAUDE_CONFIG_DIR` of the Halv process, else `~/.claude`.
pub(crate) fn system_config_dir() -> Option<PathBuf> {
    std::env::var_os(CONFIG_DIR_VARIABLE)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(default_config_dir)
}

fn default_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    (!home.is_empty()).then(|| PathBuf::from(home).join(".claude"))
}

/// The Orca variables that Halv removes from each PTY. Orca marks its panes
/// with them, and its global agent hooks report to the pane that they name.
/// A Halv started from an Orca terminal inherits them.
const ORCA_VARIABLES: [&str; 13] = [
    "ORCA_PANE_KEY",
    "ORCA_TAB_ID",
    "ORCA_WORKTREE_ID",
    "ORCA_WORKSPACE_ID",
    "ORCA_TERMINAL_HANDLE",
    "ORCA_AGENT_LAUNCH_TOKEN",
    "ORCA_AGENT_HOOK_PORT",
    "ORCA_AGENT_HOOK_TOKEN",
    "ORCA_AGENT_HOOK_ENV",
    "ORCA_AGENT_HOOK_VERSION",
    "ORCA_AGENT_HOOK_TRANSPORT",
    "ORCA_AGENT_HOOK_ENDPOINT",
    "ORCA_AGENT_HOOK_ROOT",
];

/// Every `ORCA_*` name to remove from a PTY: the known names and each one
/// in the environment of the app.
pub(crate) fn orca_variables() -> Vec<OsString> {
    orca_variables_with(std::env::vars_os().map(|(name, _)| name))
}

fn orca_variables_with(names: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut variables = ORCA_VARIABLES.map(OsString::from).to_vec();
    for name in names {
        if name.to_str().is_some_and(|name| name.starts_with("ORCA_")) && !variables.contains(&name)
        {
            variables.push(name);
        }
    }
    variables
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigDirRecord {
    dir: PathBuf,
    recorded_at: u64,
}

/// Session id to config folder, mirrored to a file in the app data folder.
#[derive(Debug, Default)]
pub(crate) struct ConfigDirRegistry {
    records: HashMap<String, ConfigDirRecord>,
    file: Option<PathBuf>,
}

impl ConfigDirRegistry {
    pub(crate) fn open(state_dir: &Path, now: u64) -> Self {
        let file = state_dir.join(REGISTRY_FILE);
        let mut records = fs::read(&file)
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<HashMap<String, ConfigDirRecord>>(&bytes).ok()
            })
            .unwrap_or_default();
        records.retain(|_, record| now.saturating_sub(record.recorded_at) < RECORD_LIFETIME_MS);
        Self {
            records,
            file: Some(file),
        }
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<&Path> {
        self.records
            .get(session_id)
            .map(|record| record.dir.as_path())
    }

    pub(crate) fn insert(&mut self, session_id: &str, dir: PathBuf, now: u64) {
        if self.get(session_id) == Some(dir.as_path()) {
            return;
        }
        self.records.insert(
            session_id.to_string(),
            ConfigDirRecord {
                dir,
                recorded_at: now,
            },
        );
        self.save();
    }

    pub(crate) fn remove(&mut self, session_id: &str) {
        if self.records.remove(session_id).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&self.records) else {
            return;
        };
        if let Some(parent) = file.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                eprintln!(
                    "agent status: could not create {}: {error}",
                    parent.display()
                );
                return;
            }
        }
        let temporary = file.with_extension("json.tmp");
        if let Err(error) = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, file))
        {
            eprintln!("agent status: could not save {}: {error}", file.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn the_launch_folder_wins_over_the_process_folder() {
        let dir = claude_config_dir_for_launch(
            &env(&[
                ("PATH", "/bin"),
                ("CLAUDE_CONFIG_DIR", "/old"),
                ("CLAUDE_CONFIG_DIR", "/accounts/claude/a1/home"),
            ]),
            &[],
        );
        assert_eq!(dir, Some(PathBuf::from("/accounts/claude/a1/home")));
    }

    #[test]
    fn a_removed_variable_means_the_default_folder() {
        let dir = claude_config_dir_for_launch(&[], &[OsString::from("CLAUDE_CONFIG_DIR")]);
        assert_eq!(dir, default_config_dir());
        assert!(dir.is_some_and(|dir| dir.ends_with(".claude")));
    }

    #[test]
    fn without_a_launch_value_the_process_folder_applies() {
        assert_eq!(claude_config_dir_for_launch(&[], &[]), system_config_dir());
    }

    #[test]
    fn orca_variables_cover_known_and_inherited_names() {
        let names = orca_variables_with(
            ["PATH", "ORCA_PANE_KEY", "ORCA_FUTURE_MARK", "HALV_X"]
                .into_iter()
                .map(OsString::from),
        );
        assert!(names.contains(&OsString::from("ORCA_AGENT_HOOK_TOKEN")));
        assert!(names.contains(&OsString::from("ORCA_FUTURE_MARK")));
        assert!(!names.contains(&OsString::from("PATH")));
        assert_eq!(
            names
                .iter()
                .filter(|name| *name == &OsString::from("ORCA_PANE_KEY"))
                .count(),
            1
        );
    }

    #[test]
    fn the_registry_survives_a_restart_and_drops_old_records() {
        let directory = std::env::temp_dir().join(format!(
            "halv-agent-status-registry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut registry = ConfigDirRegistry::open(&directory, 1_000);
        registry.insert("old", PathBuf::from("/a"), 1_000);
        registry.insert("pane", PathBuf::from("/b"), RECORD_LIFETIME_MS);
        registry.insert("gone", PathBuf::from("/c"), RECORD_LIFETIME_MS);
        registry.remove("gone");

        let reopened = ConfigDirRegistry::open(&directory, RECORD_LIFETIME_MS + 2_000);
        assert_eq!(reopened.get("pane"), Some(Path::new("/b")));
        assert_eq!(reopened.get("old"), None);
        assert_eq!(reopened.get("gone"), None);
        fs::remove_dir_all(&directory).unwrap();
    }
}
