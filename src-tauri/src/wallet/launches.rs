//! The account folder of each agent pane.
//!
//! A CLI keeps the account folder that it started with. When the active
//! account changes, a pane with another folder still runs the old account.
//! The launch records the folder of each subscription pane in a small file,
//! so Halv can find these panes also after an app restart.
//!
//! The folder decides, not the account id. Older Orca builds copy each
//! selected Claude login into the default folder, so every Claude pane
//! follows a switch there. Such a pane never needs a restart.

use super::{files, WalletProvider};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

const REGISTRY_FILE: &str = "launches.json";
/// A record older than this has no live pane. The PTY supervisor keeps
/// panes for days at most.
const RECORD_LIFETIME_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
/// The file keeps at most this many records. The oldest records go first.
const RECORD_LIMIT: usize = 512;

/// The account folder that a CLI gets at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "path")]
pub(super) enum LaunchHome {
    /// The user's own folder: no `CLAUDE_CONFIG_DIR` or `CODEX_HOME`.
    System,
    /// The exact folder text of the launch variable.
    Folder(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchRecord {
    provider: WalletProvider,
    home: LaunchHome,
    /// The active account at launch. Only the UI label uses it.
    #[serde(default)]
    account_id: Option<String>,
    /// The user chose to keep the old account while this folder was active.
    #[serde(default)]
    kept_for: Option<LaunchHome>,
    recorded_at: u64,
}

/// A live pane that runs another account than the active account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StaleLaunch {
    pub session_id: String,
    pub provider: WalletProvider,
    pub previous_account_id: Option<String>,
}

/// Session id to launch record, mirrored to a file in the wallet folder.
#[derive(Debug, Default)]
pub(super) struct LaunchRegistry {
    records: HashMap<String, LaunchRecord>,
    file: Option<PathBuf>,
}

impl LaunchRegistry {
    pub(super) fn open(root: &Path, now: u64) -> Self {
        let file = root.join(REGISTRY_FILE);
        let records = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, LaunchRecord>>(&bytes).ok())
            .unwrap_or_default();
        let mut registry = Self {
            records,
            file: Some(file),
        };
        registry.prune(now);
        registry
    }

    pub(super) fn record(
        &mut self,
        session_id: &str,
        provider: WalletProvider,
        home: LaunchHome,
        account_id: Option<String>,
        now: u64,
    ) {
        self.prune(now);
        self.records.insert(
            session_id.to_string(),
            LaunchRecord {
                provider,
                home,
                account_id,
                kept_for: None,
                recorded_at: now,
            },
        );
        self.save();
    }

    /// The live panes whose folder differs from the folder that a new launch
    /// gets now. `current` gives that folder, or `None` when Halv cannot
    /// tell. A pane without a record is never stale.
    pub(super) fn stale(
        &self,
        live: &[(String, WalletProvider)],
        current: impl Fn(WalletProvider) -> Option<LaunchHome>,
    ) -> Vec<StaleLaunch> {
        let homes = WalletProvider::ALL.map(|provider| (provider, current(provider)));
        let mut stale = live
            .iter()
            .filter_map(|(session_id, provider)| {
                let record = self.records.get(session_id)?;
                let (_, Some(home)) = homes.iter().find(|(candidate, _)| candidate == provider)?
                else {
                    return None;
                };
                let differs = record.provider == *provider
                    && record.home != *home
                    && record.kept_for.as_ref() != Some(home);
                differs.then(|| StaleLaunch {
                    session_id: session_id.clone(),
                    provider: *provider,
                    previous_account_id: record.account_id.clone(),
                })
            })
            .collect::<Vec<_>>();
        stale.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        stale
    }

    /// Marks the panes as kept on their account while `current` gives the
    /// active folder. A later switch to another folder asks again.
    pub(super) fn keep(
        &mut self,
        session_ids: &[String],
        current: impl Fn(WalletProvider) -> Option<LaunchHome>,
    ) {
        let mut changed = false;
        for session_id in session_ids {
            let Some(record) = self.records.get_mut(session_id) else {
                continue;
            };
            let Some(home) = current(record.provider) else {
                continue;
            };
            if record.kept_for.as_ref() != Some(&home) {
                record.kept_for = Some(home);
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    fn prune(&mut self, now: u64) {
        self.records
            .retain(|_, record| now.saturating_sub(record.recorded_at) < RECORD_LIFETIME_MS);
        if self.records.len() < RECORD_LIMIT {
            return;
        }
        let mut times = self
            .records
            .values()
            .map(|record| record.recorded_at)
            .collect::<Vec<_>>();
        times.sort_unstable_by(|left, right| right.cmp(left));
        // Room for one more record.
        let oldest_kept = times[RECORD_LIMIT - 2];
        self.records
            .retain(|_, record| record.recorded_at >= oldest_kept);
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let Ok(mut text) = serde_json::to_string_pretty(&self.records) else {
            return;
        };
        text.push('\n');
        let result = file
            .parent()
            .map_or(Ok(()), files::create_private_dir)
            .and_then(|()| files::write_private_file(file, text.as_bytes()));
        if let Err(error) = result {
            eprintln!("wallet: could not save the pane accounts: {error}");
        }
    }
}

/// The folder in a launch environment, as a CLI of `provider` reads it.
pub(super) fn launch_home(
    provider: WalletProvider,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> LaunchHome {
    env.iter()
        .rev()
        .find(|(name, _)| name == provider.home_variable())
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .map_or(LaunchHome::System, |value| {
            LaunchHome::Folder(PathBuf::from(value))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn folder(path: &str) -> LaunchHome {
        LaunchHome::Folder(PathBuf::from(path))
    }

    fn live(entries: &[(&str, WalletProvider)]) -> Vec<(String, WalletProvider)> {
        entries
            .iter()
            .map(|(id, provider)| (id.to_string(), *provider))
            .collect()
    }

    fn registry() -> LaunchRegistry {
        LaunchRegistry::default()
    }

    #[test]
    fn the_launch_variable_names_the_folder() {
        let env = vec![
            (OsString::from("PATH"), OsString::from("/bin")),
            (OsString::from("CODEX_HOME"), OsString::from("/old")),
            (
                OsString::from("CODEX_HOME"),
                OsString::from("/orca/codex-accounts/a/home"),
            ),
        ];
        assert_eq!(
            launch_home(WalletProvider::Codex, &env),
            folder("/orca/codex-accounts/a/home")
        );
        assert_eq!(
            launch_home(WalletProvider::Claude, &env),
            LaunchHome::System
        );
        let empty = vec![(OsString::from("CLAUDE_CONFIG_DIR"), OsString::new())];
        assert_eq!(
            launch_home(WalletProvider::Claude, &empty),
            LaunchHome::System
        );
    }

    #[test]
    fn a_pane_on_another_folder_is_stale() {
        let mut registry = registry();
        registry.record(
            "codex-a",
            WalletProvider::Codex,
            folder("/a"),
            Some("a".into()),
            1,
        );
        registry.record(
            "codex-b",
            WalletProvider::Codex,
            folder("/b"),
            Some("b".into()),
            1,
        );
        let current = |provider| match provider {
            WalletProvider::Codex => Some(folder("/b")),
            WalletProvider::Claude => Some(LaunchHome::System),
        };
        let stale = registry.stale(
            &live(&[
                ("codex-a", WalletProvider::Codex),
                ("codex-b", WalletProvider::Codex),
                ("codex-unknown", WalletProvider::Codex),
            ]),
            current,
        );
        assert_eq!(
            stale,
            vec![StaleLaunch {
                session_id: "codex-a".into(),
                provider: WalletProvider::Codex,
                previous_account_id: Some("a".into()),
            }]
        );
    }

    #[test]
    fn claude_panes_in_the_shared_default_folder_follow_a_switch() {
        // Older Orca copies each selected login into the default folder.
        // Both accounts route to System, so the pane needs no restart.
        let mut registry = registry();
        registry.record(
            "claude-1",
            WalletProvider::Claude,
            LaunchHome::System,
            Some("orca-work".into()),
            1,
        );
        let stale = registry.stale(&live(&[("claude-1", WalletProvider::Claude)]), |_| {
            Some(LaunchHome::System)
        });
        assert!(stale.is_empty());
    }

    #[test]
    fn claude_panes_under_the_orca_router_need_a_restart() {
        let mut registry = registry();
        registry.record(
            "claude-1",
            WalletProvider::Claude,
            folder("/orca/claude-profiles/work/home"),
            Some("work".into()),
            1,
        );
        let stale = registry.stale(&live(&[("claude-1", WalletProvider::Claude)]), |_| {
            Some(folder("/orca/claude-profiles/personal/home"))
        });
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].provider, WalletProvider::Claude);
    }

    #[test]
    fn an_unknown_active_folder_marks_no_pane() {
        let mut registry = registry();
        registry.record("codex-a", WalletProvider::Codex, folder("/a"), None, 1);
        let stale = registry.stale(&live(&[("codex-a", WalletProvider::Codex)]), |_| None);
        assert!(stale.is_empty());
    }

    #[test]
    fn a_switch_back_to_the_launch_folder_clears_the_pane() {
        let mut registry = registry();
        registry.record("codex-a", WalletProvider::Codex, folder("/a"), None, 1);
        let panes = live(&[("codex-a", WalletProvider::Codex)]);
        assert_eq!(registry.stale(&panes, |_| Some(folder("/b"))).len(), 1);
        assert!(registry.stale(&panes, |_| Some(folder("/a"))).is_empty());
    }

    #[test]
    fn a_kept_pane_asks_again_only_after_another_switch() {
        let mut registry = registry();
        registry.record("codex-a", WalletProvider::Codex, folder("/a"), None, 1);
        let panes = live(&[("codex-a", WalletProvider::Codex)]);
        registry.keep(&["codex-a".to_string()], |_| Some(folder("/b")));
        assert!(registry.stale(&panes, |_| Some(folder("/b"))).is_empty());
        assert_eq!(
            registry.stale(&panes, |_| Some(LaunchHome::System)).len(),
            1
        );
    }

    #[test]
    fn records_survive_a_restart_and_old_records_go() {
        let directory = std::env::temp_dir().join(format!(
            "halv-wallet-launches-{:016x}",
            rand::random::<u64>()
        ));
        let mut registry = LaunchRegistry::open(&directory, 1_000);
        registry.record(
            "codex-a",
            WalletProvider::Codex,
            folder("/a"),
            Some("a".into()),
            1_000,
        );
        registry.keep(&["codex-a".to_string()], |_| Some(folder("/b")));

        let reopened = LaunchRegistry::open(&directory, 2_000);
        let panes = live(&[("codex-a", WalletProvider::Codex)]);
        assert!(reopened.stale(&panes, |_| Some(folder("/b"))).is_empty());
        assert_eq!(reopened.stale(&panes, |_| Some(folder("/c"))).len(), 1);

        let expired = LaunchRegistry::open(&directory, RECORD_LIFETIME_MS + 2_000);
        assert!(expired.stale(&panes, |_| Some(folder("/c"))).is_empty());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_file_keeps_the_newest_records() {
        let mut registry = registry();
        for index in 0..(RECORD_LIMIT as u64 + 20) {
            registry.record(
                &format!("pane-{index}"),
                WalletProvider::Codex,
                folder("/a"),
                None,
                10 + index,
            );
        }
        assert!(registry.records.len() <= RECORD_LIMIT);
        let newest = format!("pane-{}", RECORD_LIMIT as u64 + 19);
        assert!(registry.records.contains_key(&newest));
        assert!(!registry.records.contains_key("pane-0"));
    }
}
