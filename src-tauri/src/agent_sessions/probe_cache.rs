//! Shared results for slow agent CLI checks.
//!
//! A window focus and the 5-minute refresh ask for the same checks several
//! times at once. Each check spawns a CLI, for example `claude --version` or
//! `claude auth status`. A `SharedCheck` runs one check for all concurrent
//! callers. It can also keep the result for a short time. The key of a result,
//! for example a binary fingerprint, makes a changed binary run the check
//! again at once.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime},
};

/// One check that concurrent callers share.
///
/// A caller reuses the stored result while it is younger than `ttl` and its
/// key is equal. It also takes the result of a run that started after the
/// call. A caller that arrives while an older run is in progress waits for
/// that run first. With a zero `ttl`, a caller gets only a result that is not
/// older than its call, and the callers that waited share one new run.
pub(super) struct SharedCheck<K, V> {
    ttl: Duration,
    state: Mutex<CheckState<K, V>>,
    finished: Condvar,
}

struct CheckState<K, V> {
    running: bool,
    #[cfg(test)]
    waiting: usize,
    started_runs: u64,
    stored: Option<StoredResult<K, V>>,
}

struct StoredResult<K, V> {
    key: K,
    value: V,
    stored_at: Instant,
    // The number of the run that gave this result.
    run: u64,
}

impl<K: PartialEq, V: Clone> SharedCheck<K, V> {
    pub(super) const fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            state: Mutex::new(CheckState {
                running: false,
                #[cfg(test)]
                waiting: 0,
                started_runs: 0,
                stored: None,
            }),
            finished: Condvar::new(),
        }
    }

    /// Returns a shared result for `key`, or runs `check` for all callers.
    pub(super) fn get(&self, key: K, check: impl FnOnce() -> V) -> V {
        let mut state = self.lock();
        // A run with a higher number started after this call, so its result
        // is fresh enough for any `ttl`.
        let runs_before_call = state.started_runs;
        loop {
            if let Some(stored) = &state.stored {
                let started_after_call = stored.run > runs_before_call;
                if stored.key == key
                    && (started_after_call || stored.stored_at.elapsed() < self.ttl)
                {
                    return stored.value.clone();
                }
            }
            if !state.running {
                break;
            }
            #[cfg(test)]
            {
                state.waiting += 1;
            }
            state = self
                .finished
                .wait(state)
                .expect("shared check lock poisoned");
            #[cfg(test)]
            {
                state.waiting -= 1;
            }
        }
        state.running = true;
        state.started_runs += 1;
        let run = state.started_runs;
        drop(state);

        let running = RunningCheck { check: self };
        let value = check();
        running.finish(run, key, value.clone());
        value
    }

    fn lock(&self) -> MutexGuard<'_, CheckState<K, V>> {
        self.state.lock().expect("shared check lock poisoned")
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.lock().waiting
    }
}

/// Marks a check as finished, also when the check panics.
struct RunningCheck<'a, K: PartialEq, V: Clone> {
    check: &'a SharedCheck<K, V>,
}

impl<K: PartialEq, V: Clone> RunningCheck<'_, K, V> {
    fn finish(self, run: u64, key: K, value: V) {
        let mut state = self.check.lock();
        state.stored = Some(StoredResult {
            key,
            value,
            stored_at: Instant::now(),
            run,
        });
        // `drop(self)` below clears `running` and wakes the callers that wait.
        drop(state);
    }
}

impl<K: PartialEq, V: Clone> Drop for RunningCheck<'_, K, V> {
    fn drop(&mut self) {
        let mut state = self.check.lock();
        state.running = false;
        drop(state);
        self.check.finished.notify_all();
    }
}

/// Identifies a file, for example a CLI binary.
///
/// A Claude Code update points `~/.local/bin/claude` at a new version file.
/// The resolved target, its size or its modification time changes, so the
/// fingerprint changes too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileFingerprint {
    path: PathBuf,
    target: Option<PathBuf>,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

pub(super) fn file_fingerprint(path: &Path) -> FileFingerprint {
    let metadata = fs::metadata(path).ok();
    FileFingerprint {
        path: path.to_path_buf(),
        target: fs::canonicalize(path).ok(),
        modified: metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok()),
        len: metadata.map(|metadata| metadata.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            mpsc, Arc,
        },
        thread,
    };

    const LONG_TTL: Duration = Duration::from_secs(60);

    #[test]
    fn concurrent_callers_share_one_run() {
        let check = Arc::new(SharedCheck::<&str, usize>::new(LONG_TTL));
        let runs = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let first = {
            let check = check.clone();
            let runs = runs.clone();
            thread::spawn(move || {
                check.get("claude", || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    runs.fetch_add(1, Ordering::SeqCst) + 1
                })
            })
        };
        started_rx.recv().unwrap();
        let waiters = (0..8)
            .map(|_| {
                let check = check.clone();
                let runs = runs.clone();
                thread::spawn(move || {
                    check.get("claude", || runs.fetch_add(1, Ordering::SeqCst) + 1)
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(5);
        while check.waiting() < 8 {
            assert!(Instant::now() < deadline, "the waiters did not queue");
            thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).unwrap();

        assert_eq!(first.join().unwrap(), 1);
        for waiter in waiters {
            assert_eq!(waiter.join().unwrap(), 1, "a waiter must share the run");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn with_a_zero_ttl_the_waiters_share_a_run_that_started_after_their_call() {
        let check = Arc::new(SharedCheck::<&str, usize>::new(Duration::ZERO));
        let runs = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        // The first run starts before the sign-in writes the credentials.
        let first = {
            let check = check.clone();
            let runs = runs.clone();
            thread::spawn(move || {
                check.get("claude", || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    runs.fetch_add(1, Ordering::SeqCst) + 1
                })
            })
        };
        started_rx.recv().unwrap();
        let waiters = (0..4)
            .map(|_| {
                let check = check.clone();
                let runs = runs.clone();
                thread::spawn(move || {
                    check.get("claude", || runs.fetch_add(1, Ordering::SeqCst) + 1)
                })
            })
            .collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(5);
        while check.waiting() < 4 {
            assert!(Instant::now() < deadline, "the waiters did not queue");
            thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).unwrap();

        assert_eq!(first.join().unwrap(), 1);
        for waiter in waiters {
            assert_eq!(
                waiter.join().unwrap(),
                2,
                "a waiter must not get the run that started before its call"
            );
        }
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_fresh_result_is_reused_for_the_same_key() {
        let check = SharedCheck::<&str, usize>::new(LONG_TTL);
        let runs = AtomicUsize::new(0);
        let run = || runs.fetch_add(1, Ordering::SeqCst) + 1;

        assert_eq!(check.get("claude", run), 1);
        assert_eq!(check.get("claude", run), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_expired_result_runs_the_check_again() {
        let check = SharedCheck::<&str, usize>::new(Duration::ZERO);
        let runs = AtomicUsize::new(0);
        let run = || runs.fetch_add(1, Ordering::SeqCst) + 1;

        assert_eq!(check.get("claude", run), 1);
        assert_eq!(check.get("claude", run), 2);
    }

    #[test]
    fn a_changed_key_runs_the_check_again() {
        let check = SharedCheck::<&str, usize>::new(LONG_TTL);
        let runs = AtomicUsize::new(0);
        let run = || runs.fetch_add(1, Ordering::SeqCst) + 1;

        assert_eq!(check.get("claude 2.1.299", run), 1);
        assert_eq!(check.get("claude 2.1.300", run), 2);
        assert_eq!(check.get("claude 2.1.300", run), 2);
    }

    #[test]
    fn a_caller_with_another_key_does_not_share_the_run() {
        let check = Arc::new(SharedCheck::<&str, &str>::new(Duration::ZERO));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let old_account = {
            let check = check.clone();
            thread::spawn(move || {
                check.get("work account", || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    "work login"
                })
            })
        };
        started_rx.recv().unwrap();
        let new_account = {
            let check = check.clone();
            thread::spawn(move || check.get("personal account", || "personal login"))
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while check.waiting() < 1 {
            assert!(Instant::now() < deadline, "the second caller did not queue");
            thread::sleep(Duration::from_millis(1));
        }
        release_tx.send(()).unwrap();

        assert_eq!(old_account.join().unwrap(), "work login");
        assert_eq!(new_account.join().unwrap(), "personal login");
    }

    #[test]
    fn a_panicking_check_releases_the_waiters() {
        let check = Arc::new(SharedCheck::<&str, usize>::new(LONG_TTL));
        let failed = {
            let check = check.clone();
            thread::spawn(move || check.get("claude", || panic!("probe failed")))
        };
        assert!(failed.join().is_err());

        assert_eq!(check.get("claude", || 7), 7);
    }

    #[cfg(unix)]
    #[test]
    fn the_fingerprint_changes_when_the_binary_changes() {
        let directory = std::env::temp_dir().join(format!(
            "halv-probe-fingerprint-{}",
            crate::storage::new_uuid_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let first_version = directory.join("2.1.299");
        let second_version = directory.join("2.1.300");
        fs::write(&first_version, "old binary").unwrap();
        fs::write(&second_version, "new binary, longer").unwrap();
        let link = directory.join("claude");
        std::os::unix::fs::symlink(&first_version, &link).unwrap();

        let before = file_fingerprint(&link);
        assert_eq!(file_fingerprint(&link), before);

        // An auto-update points the link at the new version file.
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&second_version, &link).unwrap();
        let after_update = file_fingerprint(&link);
        assert_ne!(after_update, before);

        // An in-place rewrite changes the size of the same file.
        fs::write(&second_version, "rewritten").unwrap();
        assert_ne!(file_fingerprint(&link), after_update);

        fs::remove_dir_all(&directory).unwrap();
    }
}
