use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const BOOT_RECORD_FILE_NAME: &str = "boot-record-v1.json";
const BOOT_RECORD_VERSION: u8 = 1;
const MAX_RUNS: usize = 50;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShutdownKind {
    Clean,
    Unclean,
    Running,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BootRecordEntry {
    pub boot_id: String,
    pub app_version: String,
    pub started_at: String,
    pub last_alive_at: String,
    pub shutdown_at: Option<String>,
    pub shutdown_kind: ShutdownKind,
    pub max_rss_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootRecordFile {
    version: u8,
    runs: Vec<BootRecordEntry>,
}

impl Default for BootRecordFile {
    fn default() -> Self {
        Self {
            version: BOOT_RECORD_VERSION,
            runs: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct BootRecordState {
    file: BootRecordFile,
    current_boot_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BootRecordManager {
    path: PathBuf,
    state: Arc<Mutex<BootRecordState>>,
}

impl BootRecordManager {
    pub fn open(app_data_dir: &Path) -> Self {
        let _ = fs::create_dir_all(app_data_dir);
        let path = app_data_dir.join(BOOT_RECORD_FILE_NAME);
        let file = fs::read(&path)
            .ok()
            .and_then(|contents| serde_json::from_slice::<BootRecordFile>(&contents).ok())
            .filter(|file| file.version == BOOT_RECORD_VERSION)
            .unwrap_or_default();
        Self {
            path,
            state: Arc::new(Mutex::new(BootRecordState {
                file,
                current_boot_id: None,
            })),
        }
    }

    fn persist(&self, file: &BootRecordFile) {
        // A persist failure is not fatal. Log it so gaps in crash-loop
        // detection are visible.
        if let Err(error) = persist_file(&self.path, file) {
            eprintln!(
                "boot record: persist failed for `{}`: {error}",
                self.path.display()
            );
        }
    }

    pub fn start_run(&self, app_version: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for entry in &mut state.file.runs {
            if entry.shutdown_kind == ShutdownKind::Running {
                entry.shutdown_kind = ShutdownKind::Unclean;
                entry.shutdown_at = Some(entry.last_alive_at.clone());
            }
        }

        let now = timestamp();
        let boot_id = format!("{:032x}", rand::random::<u128>());
        state.file.runs.push(BootRecordEntry {
            boot_id: boot_id.clone(),
            app_version: app_version.to_string(),
            started_at: now.clone(),
            last_alive_at: now,
            shutdown_at: None,
            shutdown_kind: ShutdownKind::Running,
            max_rss_bytes: process_max_rss_bytes(),
        });
        trim_old_runs(&mut state.file.runs);
        state.current_boot_id = Some(boot_id);
        self.persist(&state.file);
    }

    pub fn heartbeat(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(current_boot_id) = state.current_boot_id.clone() else {
            return;
        };
        let Some(entry) = state
            .file
            .runs
            .iter_mut()
            .find(|entry| entry.boot_id == current_boot_id)
        else {
            return;
        };
        if entry.shutdown_kind != ShutdownKind::Running {
            return;
        }
        entry.last_alive_at = timestamp();
        if let Some(max_rss_bytes) = process_max_rss_bytes() {
            entry.max_rss_bytes = Some(max_rss_bytes);
        }
        self.persist(&state.file);
    }

    pub fn mark_clean_shutdown(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(current_boot_id) = state.current_boot_id.clone() else {
            return;
        };
        let Some(entry) = state
            .file
            .runs
            .iter_mut()
            .find(|entry| entry.boot_id == current_boot_id)
        else {
            return;
        };
        if entry.shutdown_kind != ShutdownKind::Running {
            return;
        }
        let now = timestamp();
        entry.last_alive_at = now.clone();
        entry.shutdown_at = Some(now);
        entry.shutdown_kind = ShutdownKind::Clean;
        if let Some(max_rss_bytes) = process_max_rss_bytes() {
            entry.max_rss_bytes = Some(max_rss_bytes);
        }
        self.persist(&state.file);
    }

    pub fn history(&self, limit: usize) -> Vec<BootRecordEntry> {
        let Ok(state) = self.state.lock() else {
            return Vec::new();
        };
        state.file.runs.iter().rev().take(limit).cloned().collect()
    }
}

pub fn spawn_heartbeat(manager: BootRecordManager) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let manager = manager.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || manager.heartbeat()).await;
        }
    });
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn trim_old_runs(runs: &mut Vec<BootRecordEntry>) {
    if runs.len() > MAX_RUNS {
        runs.drain(..runs.len() - MAX_RUNS);
    }
}

fn persist_file(path: &Path, file: &BootRecordFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut serialized = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    serialized.push(b'\n');

    let temporary_path = path.with_extension("json.tmp");
    let result = (|| {
        let mut temporary = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&serialized)?;
        temporary.sync_all()?;
        drop(temporary);
        replace_file(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_file(temporary_path: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn process_max_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let max_rss = unsafe { usage.assume_init().ru_maxrss };
    if max_rss < 0 {
        return None;
    }
    let max_rss = max_rss as u64;

    // Normalize units because macOS reports bytes and Linux reports kibibytes.
    #[cfg(target_os = "macos")]
    return Some(max_rss);

    #[cfg(not(target_os = "macos"))]
    max_rss.checked_mul(1024)
}

#[cfg(not(unix))]
fn process_max_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "halv-boot-record-{label}-{:032x}",
            rand::random::<u128>()
        ))
    }

    #[test]
    fn reopening_marks_a_running_entry_unclean() {
        let root = temp_dir("unclean");
        let first = BootRecordManager::open(&root);
        first.start_run("1.0.0");
        let first_entry = first.history(1).pop().expect("run should exist");
        drop(first);

        let second = BootRecordManager::open(&root);
        second.start_run("1.0.1");
        let history = second.history(2);
        let previous = &history[1];

        assert_eq!(previous.boot_id, first_entry.boot_id);
        assert_eq!(previous.shutdown_kind, ShutdownKind::Unclean);
        assert_eq!(previous.shutdown_at.as_ref(), Some(&previous.last_alive_at));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clean_shutdown_is_written() {
        let root = temp_dir("clean");
        let manager = BootRecordManager::open(&root);
        manager.start_run("1.0.0");
        manager.mark_clean_shutdown();
        drop(manager);

        let reopened = BootRecordManager::open(&root);
        let entry = reopened.history(1).pop().expect("run should exist");
        assert_eq!(entry.shutdown_kind, ShutdownKind::Clean);
        assert_eq!(entry.shutdown_at.as_ref(), Some(&entry.last_alive_at));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn history_drops_the_oldest_runs_above_the_limit() {
        let root = temp_dir("limit");
        let manager = BootRecordManager::open(&root);
        for index in 0..52 {
            manager.start_run(&format!("version-{index}"));
        }
        let history = manager.history(100);

        assert_eq!(history.len(), MAX_RUNS);
        assert_eq!(history[0].app_version, "version-51");
        assert_eq!(history[MAX_RUNS - 1].app_version, "version-2");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_and_corrupt_files_open_as_empty_records() {
        let root = temp_dir("invalid");
        let missing = BootRecordManager::open(&root);
        assert!(missing.history(10).is_empty());
        drop(missing);

        fs::write(root.join(BOOT_RECORD_FILE_NAME), b"not json")
            .expect("corrupt fixture should write");
        let corrupt = BootRecordManager::open(&root);
        assert!(corrupt.history(10).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
