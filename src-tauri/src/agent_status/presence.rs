//! Reader for the Claude Code session presence file.
//!
//! Each interactive Claude Code process writes
//! `<config dir>/sessions/<pid>.json` and rewrites it on each status change.
//! The file holds `status` (`busy`, `idle`, `shell`, or `waiting`) and, while
//! the session waits, `waitingFor`. The file is internal Claude Code state,
//! so the reader accepts unknown fields and ignores unknown values.
//!
//! The reader opens only `<pid>.json`. It never lists the folder and never
//! opens the `.key` files beside the presence files.

use super::machine::PresenceState;
use serde_json::Value;
use std::{
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A presence file is about 1 KiB. A larger file is not a presence file.
const MAX_PRESENCE_BYTES: u64 = 64 * 1024;
/// Claude registers its presence a moment after the PTY starts. A file that
/// claims an earlier start belongs to an older process with the same PID.
const START_TOLERANCE_MS: u64 = 5_000;

/// File access for the reader. Tests replace it to prove which paths the
/// reader opens.
pub(crate) trait PresenceFiles {
    fn fingerprint(&self, path: &Path) -> io::Result<(SystemTime, u64)>;
    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>>;
}

pub(crate) struct DiskFiles;

impl PresenceFiles for DiskFiles {
    fn fingerprint(&self, path: &Path) -> io::Result<(SystemTime, u64)> {
        let metadata = fs::metadata(path)?;
        Ok((metadata.modified()?, metadata.len()))
    }

    fn read(&self, path: &Path, limit: u64) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        fs::File::open(path)?.take(limit).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

pub(crate) fn presence_path(config_dir: &Path, pid: u32) -> PathBuf {
    config_dir.join("sessions").join(format!("{pid}.json"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PresenceRead {
    /// The file did not change since the last read.
    Unchanged,
    /// No valid file exists for this process.
    Missing,
    State(PresenceState),
}

#[derive(Debug, PartialEq, Eq)]
enum ParseFailure {
    /// The file is not complete yet. The next poll reads it again.
    Retry,
    /// The file belongs to another process or holds no usable status.
    Foreign,
    /// The status value is new to this reader.
    Unknown,
}

/// Polls one presence file. It parses the file only when its modification
/// time or size changes.
#[derive(Debug)]
pub(crate) struct PresenceProbe {
    config_dir: PathBuf,
    path: PathBuf,
    pid: u32,
    min_started_at: u64,
    fingerprint: Option<(SystemTime, u64)>,
    found: bool,
}

impl PresenceProbe {
    pub(crate) fn new(config_dir: &Path, pid: u32, pane_started_at: u64) -> Self {
        Self {
            config_dir: config_dir.to_path_buf(),
            path: presence_path(config_dir, pid),
            pid,
            min_started_at: pane_started_at.saturating_sub(START_TOLERANCE_MS),
            fingerprint: None,
            found: false,
        }
    }

    pub(crate) fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    #[cfg(test)]
    fn found(&self) -> bool {
        self.found
    }

    pub(crate) fn poll(&mut self, files: &dyn PresenceFiles) -> PresenceRead {
        let fingerprint = match files.fingerprint(&self.path) {
            Ok(fingerprint) => fingerprint,
            Err(_) => return self.lost(),
        };
        if self.fingerprint == Some(fingerprint) {
            return PresenceRead::Unchanged;
        }
        if fingerprint.1 > MAX_PRESENCE_BYTES {
            return self.lost();
        }
        let Ok(bytes) = files.read(&self.path, MAX_PRESENCE_BYTES) else {
            return self.lost();
        };
        match parse_presence(&bytes, self.pid, self.min_started_at) {
            Ok(state) => {
                self.fingerprint = Some(fingerprint);
                self.found = true;
                PresenceRead::State(state)
            }
            // The fingerprint stays unset, so the next poll reads again.
            Err(ParseFailure::Retry) => PresenceRead::Unchanged,
            Err(ParseFailure::Unknown) => {
                self.fingerprint = Some(fingerprint);
                PresenceRead::Unchanged
            }
            Err(ParseFailure::Foreign) => {
                self.fingerprint = Some(fingerprint);
                self.lose()
            }
        }
    }

    fn lost(&mut self) -> PresenceRead {
        self.fingerprint = None;
        self.lose()
    }

    fn lose(&mut self) -> PresenceRead {
        if std::mem::take(&mut self.found) {
            PresenceRead::Missing
        } else {
            PresenceRead::Unchanged
        }
    }
}

fn parse_presence(
    bytes: &[u8],
    pid: u32,
    min_started_at: u64,
) -> Result<PresenceState, ParseFailure> {
    let value = serde_json::from_slice::<Value>(bytes).map_err(|_| ParseFailure::Retry)?;
    let object = value.as_object().ok_or(ParseFailure::Foreign)?;
    if object.get("pid").and_then(Value::as_u64) != Some(u64::from(pid)) {
        return Err(ParseFailure::Foreign);
    }
    if let Some(started_at) = object.get("startedAt").and_then(Value::as_u64) {
        if started_at < min_started_at {
            return Err(ParseFailure::Foreign);
        }
    }
    match object.get("status").and_then(Value::as_str) {
        Some("busy") => Ok(PresenceState::Busy),
        Some("idle" | "shell") => Ok(PresenceState::Rest),
        Some("waiting") => Ok(PresenceState::Waiting(
            object
                .get("waitingFor")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )),
        Some(_) => Err(ParseFailure::Unknown),
        None => Err(ParseFailure::Foreign),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        collections::HashMap,
        time::{Duration, UNIX_EPOCH},
    };

    /// In-memory files that record each path the reader touches.
    #[derive(Default)]
    struct FakeFiles {
        files: RefCell<HashMap<PathBuf, (SystemTime, Vec<u8>)>>,
        touched: RefCell<Vec<PathBuf>>,
        reads: RefCell<usize>,
    }

    impl FakeFiles {
        fn write(&self, path: &Path, modified_secs: u64, contents: &str) {
            self.files.borrow_mut().insert(
                path.to_path_buf(),
                (
                    UNIX_EPOCH + Duration::from_secs(modified_secs),
                    contents.as_bytes().to_vec(),
                ),
            );
        }

        fn remove(&self, path: &Path) {
            self.files.borrow_mut().remove(path);
        }
    }

    impl PresenceFiles for FakeFiles {
        fn fingerprint(&self, path: &Path) -> io::Result<(SystemTime, u64)> {
            self.touched.borrow_mut().push(path.to_path_buf());
            self.files
                .borrow()
                .get(path)
                .map(|(modified, bytes)| (*modified, bytes.len() as u64))
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn read(&self, path: &Path, _limit: u64) -> io::Result<Vec<u8>> {
            self.touched.borrow_mut().push(path.to_path_buf());
            *self.reads.borrow_mut() += 1;
            self.files
                .borrow()
                .get(path)
                .map(|(_, bytes)| bytes.clone())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    const CONFIG: &str = "/home/dev/.claude";

    fn record(pid: u32, started_at: u64, status: &str, waiting_for: Option<&str>) -> String {
        let mut value = serde_json::json!({
            "pid": pid,
            "sessionId": "8c1f",
            "cwd": "/repo",
            "startedAt": started_at,
            "kind": "interactive",
            "status": status,
            "statusUpdatedAt": started_at + 10,
            "version": "2.1.299",
            "futureField": { "nested": true },
        });
        if let Some(waiting_for) = waiting_for {
            value["waitingFor"] = serde_json::json!(waiting_for);
        }
        value.to_string()
    }

    #[test]
    fn the_path_uses_the_pane_pid_inside_the_sessions_folder() {
        assert_eq!(
            presence_path(Path::new(CONFIG), 4311),
            PathBuf::from("/home/dev/.claude/sessions/4311.json")
        );
    }

    #[test]
    fn statuses_map_to_live_states() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 1_000_000);
        for (second, status, waiting_for, expected) in [
            (1, "busy", None, PresenceState::Busy),
            (2, "idle", None, PresenceState::Rest),
            (3, "shell", None, PresenceState::Rest),
            (
                4,
                "waiting",
                Some("permission prompt"),
                PresenceState::Waiting("permission prompt".to_string()),
            ),
            (
                5,
                "waiting",
                Some("input needed"),
                PresenceState::Waiting("input needed".to_string()),
            ),
        ] {
            files.write(&path, second, &record(77, 1_000_000, status, waiting_for));
            assert_eq!(
                probe.poll(&files),
                PresenceRead::State(expected),
                "{status}"
            );
        }
        assert!(probe.found());
    }

    #[test]
    fn an_unchanged_file_is_not_parsed_again() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 0);
        files.write(&path, 1, &record(77, 10, "busy", None));
        assert_eq!(probe.poll(&files), PresenceRead::State(PresenceState::Busy));
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        assert_eq!(*files.reads.borrow(), 1);
    }

    #[test]
    fn a_partial_write_is_read_again_on_the_next_poll() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 0);
        files.write(&path, 1, "{\"pid\":77,\"status\":\"bu");
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        // The writer finishes within the same modification time.
        files.write(&path, 1, &record(77, 10, "busy", None));
        assert_eq!(probe.poll(&files), PresenceRead::State(PresenceState::Busy));
    }

    #[test]
    fn a_file_from_another_process_is_rejected() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let pane_started_at = 2_000_000;
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, pane_started_at);
        // Another PID wrote this file.
        files.write(&path, 1, &record(78, pane_started_at, "busy", None));
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        // A stale file from an older process with the same PID.
        files.write(
            &path,
            2,
            &record(77, pane_started_at - 60_000, "busy", None),
        );
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        assert!(!probe.found());
        // A start a moment before the pane start is the same process.
        files.write(&path, 3, &record(77, pane_started_at - 1_000, "idle", None));
        assert_eq!(probe.poll(&files), PresenceRead::State(PresenceState::Rest));
    }

    #[test]
    fn a_missing_file_is_reported_once_after_it_was_found() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 0);
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        files.write(&path, 1, &record(77, 10, "busy", None));
        assert_eq!(probe.poll(&files), PresenceRead::State(PresenceState::Busy));
        files.remove(&path);
        assert_eq!(probe.poll(&files), PresenceRead::Missing);
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        files.write(&path, 2, &record(77, 10, "idle", None));
        assert_eq!(probe.poll(&files), PresenceRead::State(PresenceState::Rest));
    }

    #[test]
    fn unknown_status_values_change_nothing() {
        let files = FakeFiles::default();
        let path = presence_path(Path::new(CONFIG), 77);
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 0);
        files.write(&path, 1, &record(77, 10, "busy", None));
        probe.poll(&files);
        files.write(&path, 2, &record(77, 10, "hibernating", None));
        assert_eq!(probe.poll(&files), PresenceRead::Unchanged);
        assert!(probe.found());
        files.write(&path, 3, "[1, 2, 3]");
        assert_eq!(probe.poll(&files), PresenceRead::Missing);
    }

    #[test]
    fn the_reader_opens_only_the_pane_json_file() {
        let files = FakeFiles::default();
        let sessions = Path::new(CONFIG).join("sessions");
        files.write(&sessions.join("77.0f3a9c.key"), 1, "secret");
        files.write(&sessions.join("78.json"), 1, &record(78, 10, "busy", None));
        files.write(&sessions.join("77.json"), 1, &record(77, 10, "busy", None));
        let mut probe = PresenceProbe::new(Path::new(CONFIG), 77, 0);
        for _ in 0..3 {
            probe.poll(&files);
        }
        let touched = files.touched.borrow();
        assert!(!touched.is_empty());
        assert!(touched.iter().all(|path| path == &sessions.join("77.json")));
    }

    #[test]
    fn the_disk_reader_reads_a_real_file() {
        let directory = std::env::temp_dir().join(format!(
            "halv-presence-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(directory.join("sessions")).unwrap();
        fs::write(
            presence_path(&directory, 91),
            record(91, 10, "waiting", Some("permission prompt")),
        )
        .unwrap();
        let mut probe = PresenceProbe::new(&directory, 91, 0);
        assert_eq!(
            probe.poll(&DiskFiles),
            PresenceRead::State(PresenceState::Waiting("permission prompt".to_string()))
        );
        assert_eq!(probe.poll(&DiskFiles), PresenceRead::Unchanged);
        fs::remove_dir_all(&directory).unwrap();
        assert_eq!(probe.poll(&DiskFiles), PresenceRead::Missing);
    }
}
