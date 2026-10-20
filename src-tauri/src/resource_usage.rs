//! Memory of Halv and its terminals for the status bar.
//!
//! The probe samples every process once, then sums each process tree. A
//! terminal tree starts at the shell the session spawned. The app tree
//! starts at this process and skips the terminal trees, so the supervisor,
//! the sidecars and the webview count once.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::terminal::TerminalManager;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalResourceUsage {
    pub session_id: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsageSnapshot {
    pub app_bytes: u64,
    pub terminals: Vec<TerminalResourceUsage>,
    pub total_bytes: u64,
}

/// One sampled process: its id, its parent and its resident memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessSample {
    pub pid: u32,
    pub parent: Option<u32>,
    pub bytes: u64,
}

/// Keeps one `System` so each refresh reuses its process table.
#[derive(Clone, Default)]
pub struct ResourceUsageProbe {
    system: Arc<Mutex<Option<System>>>,
}

impl ResourceUsageProbe {
    fn sample(&self) -> Vec<ProcessSample> {
        let mut guard = match self.system.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let system = guard.get_or_insert_with(System::new);
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        system
            .processes()
            .iter()
            .map(|(pid, process)| ProcessSample {
                pid: pid.as_u32(),
                parent: process.parent().map(|parent| parent.as_u32()),
                bytes: process.memory(),
            })
            .collect()
    }
}

fn process_tree(root: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut queue = vec![root];
    let mut tree = Vec::new();
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        tree.push(pid);
        if let Some(kids) = children.get(&pid) {
            queue.extend(kids.iter().copied());
        }
    }
    tree
}

/// Sums the trees. A process that two trees share counts in the first tree.
pub(crate) fn snapshot_from_samples(
    samples: &[ProcessSample],
    app_pid: u32,
    terminals: &[(String, u32)],
) -> ResourceUsageSnapshot {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut bytes_of: HashMap<u32, u64> = HashMap::new();
    for sample in samples {
        bytes_of.insert(sample.pid, sample.bytes);
        if let Some(parent) = sample.parent {
            if parent != sample.pid {
                children.entry(parent).or_default().push(sample.pid);
            }
        }
    }
    let mut counted: HashSet<u32> = HashSet::new();
    let mut sum_tree = |root: u32| -> u64 {
        process_tree(root, &children)
            .into_iter()
            .filter(|pid| counted.insert(*pid))
            .filter_map(|pid| bytes_of.get(&pid).copied())
            .sum()
    };
    let terminal_usage = terminals
        .iter()
        .map(|(session_id, pid)| TerminalResourceUsage {
            session_id: session_id.clone(),
            bytes: sum_tree(*pid),
        })
        .collect::<Vec<_>>();
    let app_bytes = sum_tree(app_pid);
    let total_bytes = app_bytes
        + terminal_usage
            .iter()
            .map(|terminal| terminal.bytes)
            .sum::<u64>();
    ResourceUsageSnapshot {
        app_bytes,
        terminals: terminal_usage,
        total_bytes,
    }
}

#[tauri::command]
pub async fn diagnostics_resource_usage(
    probe: tauri::State<'_, ResourceUsageProbe>,
    manager: tauri::State<'_, TerminalManager>,
) -> Result<ResourceUsageSnapshot, String> {
    let terminals = manager
        .list()
        .into_iter()
        .filter(|session| session.status == "running" && session.pid > 0)
        .map(|session| (session.id, session.pid))
        .collect::<Vec<_>>();
    let probe = probe.inner().clone();
    let samples = tauri::async_runtime::spawn_blocking(move || probe.sample())
        .await
        .map_err(|error| format!("failed to sample processes: {error}"))?;
    Ok(snapshot_from_samples(
        &samples,
        std::process::id(),
        &terminals,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, parent: Option<u32>, bytes: u64) -> ProcessSample {
        ProcessSample { pid, parent, bytes }
    }

    #[test]
    fn sums_each_tree_once_and_keeps_terminals_out_of_the_app_tree() {
        // 1 is the app. 2 is a sidecar. 10 is a shell under the app, with
        // child 11. 20 is a shell under a detached supervisor 19.
        let samples = vec![
            sample(1, None, 100),
            sample(2, Some(1), 50),
            sample(10, Some(1), 30),
            sample(11, Some(10), 70),
            sample(19, None, 5),
            sample(20, Some(19), 40),
            sample(21, Some(20), 60),
            sample(99, Some(98), 1000),
        ];
        let snapshot =
            snapshot_from_samples(&samples, 1, &[("a".to_string(), 10), ("b".to_string(), 20)]);
        assert_eq!(
            snapshot,
            ResourceUsageSnapshot {
                app_bytes: 150,
                terminals: vec![
                    TerminalResourceUsage {
                        session_id: "a".to_string(),
                        bytes: 100
                    },
                    TerminalResourceUsage {
                        session_id: "b".to_string(),
                        bytes: 100
                    },
                ],
                total_bytes: 350,
            }
        );
    }

    #[test]
    fn survives_parent_cycles_and_missing_processes() {
        let samples = vec![
            sample(1, Some(2), 10),
            sample(2, Some(1), 20),
            sample(3, Some(3), 30),
        ];
        let snapshot = snapshot_from_samples(&samples, 1, &[("gone".to_string(), 404)]);
        assert_eq!(snapshot.app_bytes, 30);
        assert_eq!(snapshot.terminals[0].bytes, 0);
        assert_eq!(snapshot.total_bytes, 30);
    }
}
