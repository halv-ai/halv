//! In-memory host parts for tests: a backend, a device registry, and an approver.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::broadcast;

use crate::host::{
    BoxFuture, Chunk, DeviceStore, HostBackend, NewDevice, OutputEvent, PairRequestInfo,
    PairingApprover, Replay, Snapshot, StoreError, StoredDevice, WriteError,
};
use crate::ids::DeviceId;
use crate::rpc::{HostInfo, Pane, PaneKind, PaneState};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// The default ring size: 256 KiB, as the desktop ring (ADR 0008).
pub const DEFAULT_RING_BYTES: usize = 256 * 1024;

/// The capacity of the output channel of a [`MemoryBackend`].
const OUTPUT_CAPACITY: usize = 16 * 1024;

/// The largest write that the backend takes, as the desktop terminal manager (64 KiB).
pub const MAX_WRITE_BYTES: usize = 64 * 1024;

struct MemoryPane {
    pane: Pane,
    chunks: VecDeque<Chunk>,
    bytes: usize,
    seq: u64,
    /// The newest sequence number that left the ring.
    trimmed_to: u64,
}

struct BackendState {
    panes: BTreeMap<String, MemoryPane>,
    ring_bytes: usize,
    writes: Vec<(String, Vec<u8>)>,
    replays: Vec<(String, u64)>,
    agent_status: bool,
    bracketed_paste: BTreeMap<String, bool>,
}

/// A [`HostBackend`] with panes in memory.
///
/// A test adds panes and output, and reads what the engine wrote.
/// It follows the rules of the trait: the ring gets each chunk before the output event.
/// Like the desktop, it refuses a write above [`MAX_WRITE_BYTES`].
pub struct MemoryBackend {
    info: HostInfo,
    state: Mutex<BackendState>,
    output: broadcast::Sender<OutputEvent>,
    changes: broadcast::Sender<()>,
}

impl std::fmt::Debug for MemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryBackend")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl MemoryBackend {
    /// Makes a backend for a host with this name.
    pub fn new(host_name: &str) -> Arc<MemoryBackend> {
        let (output, _) = broadcast::channel(OUTPUT_CAPACITY);
        let (changes, _) = broadcast::channel(64);
        Arc::new(MemoryBackend {
            info: HostInfo {
                name: host_name.to_owned(),
                app: "halv-desktop/0.8.0".to_owned(),
                os: "test".to_owned(),
            },
            state: Mutex::new(BackendState {
                panes: BTreeMap::new(),
                ring_bytes: DEFAULT_RING_BYTES,
                writes: Vec::new(),
                replays: Vec::new(),
                agent_status: false,
                bracketed_paste: BTreeMap::new(),
            }),
            output,
            changes,
        })
    }

    /// Adds a running shell pane.
    pub fn add_pane(&self, session: &str, cols: u16, rows: u16) {
        let pane = Pane {
            session: session.to_owned(),
            title: format!("shell {session}"),
            kind: PaneKind::Shell,
            project: None,
            cwd: None,
            cols,
            rows,
            state: PaneState::Running,
            exit_code: None,
            last_output_at: None,
            status: None,
        };
        lock(&self.state).panes.insert(
            session.to_owned(),
            MemoryPane {
                pane,
                chunks: VecDeque::new(),
                bytes: 0,
                seq: 0,
                trimmed_to: 0,
            },
        );
        let _ = self.changes.send(());
    }

    /// Removes a pane. The engine sends `terminal.closed` and a new pane list.
    pub fn remove_pane(&self, session: &str) {
        lock(&self.state).panes.remove(session);
        let _ = self.output.send(OutputEvent::Closed {
            session: session.to_owned(),
        });
        let _ = self.changes.send(());
    }

    /// Appends output to a pane and returns its sequence number.
    pub fn push_output(&self, session: &str, data: &[u8]) -> u64 {
        let seq = {
            let mut state = lock(&self.state);
            let limit = state.ring_bytes;
            let Some(pane) = state.panes.get_mut(session) else {
                return 0;
            };
            pane.seq += 1;
            pane.chunks.push_back(Chunk {
                seq: pane.seq,
                data: data.to_vec(),
            });
            pane.bytes += data.len();
            while pane.bytes > limit && pane.chunks.len() > 1 {
                if let Some(chunk) = pane.chunks.pop_front() {
                    pane.bytes -= chunk.data.len();
                    pane.trimmed_to = chunk.seq;
                }
            }
            pane.seq
        };
        let _ = self.output.send(OutputEvent::Data {
            session: session.to_owned(),
            seq,
            data: Arc::from(data),
        });
        seq
    }

    /// Changes the PTY size of a pane.
    pub fn resize(&self, session: &str, cols: u16, rows: u16) {
        if let Some(pane) = lock(&self.state).panes.get_mut(session) {
            pane.pane.cols = cols;
            pane.pane.rows = rows;
        }
        let _ = self.output.send(OutputEvent::Resized {
            session: session.to_owned(),
            cols,
            rows,
        });
    }

    /// Marks the process of a pane as exited.
    pub fn exit(&self, session: &str, code: Option<i32>) {
        if let Some(pane) = lock(&self.state).panes.get_mut(session) {
            pane.pane.state = PaneState::Exited;
            pane.pane.exit_code = code;
        }
        let _ = self.output.send(OutputEvent::Exit {
            session: session.to_owned(),
            code,
            signal: None,
        });
        let _ = self.changes.send(());
    }

    /// Sets the ring size of every pane.
    pub fn set_ring_bytes(&self, bytes: usize) {
        lock(&self.state).ring_bytes = bytes;
    }

    /// Turns the `agent-status.v1` capability on or off.
    pub fn set_agent_status(&self, on: bool) {
        lock(&self.state).agent_status = on;
    }

    /// Gives the engine a bracketed paste mode for a pane, as a backend that knows it.
    pub fn set_bracketed_paste(&self, session: &str, on: bool) {
        lock(&self.state)
            .bracketed_paste
            .insert(session.to_owned(), on);
    }

    /// Returns the input that the engine wrote, in order: `(session, bytes)`.
    pub fn writes(&self) -> Vec<(String, Vec<u8>)> {
        lock(&self.state).writes.clone()
    }

    /// Returns the replays that the engine asked for: `(session, after_seq)`.
    pub fn replay_requests(&self) -> Vec<(String, u64)> {
        lock(&self.state).replays.clone()
    }
}

impl HostBackend for MemoryBackend {
    fn host_info(&self) -> HostInfo {
        self.info.clone()
    }

    fn agent_status(&self) -> bool {
        lock(&self.state).agent_status
    }

    fn panes(&self) -> Vec<Pane> {
        lock(&self.state)
            .panes
            .values()
            .map(|pane| pane.pane.clone())
            .collect()
    }

    fn pane_changes(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    fn snapshot(&self, session: &str) -> Option<Snapshot> {
        let state = lock(&self.state);
        let pane = state.panes.get(session)?;
        let mut data = Vec::with_capacity(pane.bytes);
        for chunk in &pane.chunks {
            data.extend_from_slice(&chunk.data);
        }
        Some(Snapshot {
            seq: pane.seq,
            cols: pane.pane.cols,
            rows: pane.pane.rows,
            data,
        })
    }

    fn replay_since(&self, session: &str, after_seq: u64) -> Option<Replay> {
        let mut state = lock(&self.state);
        state.replays.push((session.to_owned(), after_seq));
        let pane = state.panes.get(session)?;
        if after_seq > pane.seq || after_seq < pane.trimmed_to {
            return None;
        }
        Some(Replay {
            cols: pane.pane.cols,
            rows: pane.pane.rows,
            chunks: pane
                .chunks
                .iter()
                .filter(|chunk| chunk.seq > after_seq)
                .cloned()
                .collect(),
        })
    }

    fn output(&self) -> broadcast::Receiver<OutputEvent> {
        self.output.subscribe()
    }

    fn write(&self, session: &str, data: &[u8]) -> Result<(), WriteError> {
        let mut state = lock(&self.state);
        if !state.panes.contains_key(session) {
            return Err(WriteError::NotFound);
        }
        if data.len() > MAX_WRITE_BYTES {
            return Err(WriteError::Failed(
                "the write is larger than 64 KiB".to_owned(),
            ));
        }
        state.writes.push((session.to_owned(), data.to_vec()));
        Ok(())
    }

    fn bracketed_paste(&self, session: &str) -> Option<bool> {
        lock(&self.state).bracketed_paste.get(session).copied()
    }
}

/// A [`DeviceStore`] in memory.
#[derive(Debug, Default)]
pub struct MemoryDeviceStore {
    devices: Mutex<Vec<StoredDevice>>,
}

impl MemoryDeviceStore {
    /// Makes an empty registry.
    pub fn new() -> Arc<MemoryDeviceStore> {
        Arc::new(MemoryDeviceStore::default())
    }

    /// Returns every device.
    pub fn devices(&self) -> Vec<StoredDevice> {
        lock(&self.devices).clone()
    }

    /// Returns one device.
    pub fn get(&self, id: &DeviceId) -> Option<StoredDevice> {
        lock(&self.devices)
            .iter()
            .find(|device| device.id == *id)
            .cloned()
    }
}

impl DeviceStore for MemoryDeviceStore {
    fn list(&self) -> Result<Vec<StoredDevice>, StoreError> {
        Ok(self.devices())
    }

    fn find_by_noise_key(&self, key: &[u8; 32]) -> Result<Option<StoredDevice>, StoreError> {
        Ok(lock(&self.devices)
            .iter()
            .find(|device| device.noise_public == *key)
            .cloned())
    }

    fn insert(&self, device: NewDevice) -> Result<StoredDevice, StoreError> {
        let mut devices = lock(&self.devices);
        if devices
            .iter()
            .any(|stored| stored.id == device.id || stored.noise_public == device.noise_public)
        {
            return Err(StoreError("the device is already stored".to_owned()));
        }
        let stored = StoredDevice {
            id: device.id,
            name: device.name,
            platform: device.platform,
            model: device.model,
            noise_public: device.noise_public,
            token_hash: device.token_hash,
            created_at_ms: device.created_at_ms,
            last_seen_at_ms: None,
            revoked_at_ms: None,
        };
        devices.push(stored.clone());
        Ok(stored)
    }

    fn revoke(&self, id: &DeviceId, at_ms: u64) -> Result<bool, StoreError> {
        let mut devices = lock(&self.devices);
        match devices.iter_mut().find(|device| device.id == *id) {
            Some(device) => {
                device.revoked_at_ms.get_or_insert(at_ms);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn touch_last_seen(&self, id: &DeviceId, at_ms: u64) -> Result<(), StoreError> {
        if let Some(device) = lock(&self.devices)
            .iter_mut()
            .find(|device| device.id == *id)
        {
            device.last_seen_at_ms = Some(at_ms);
        }
        Ok(())
    }
}

/// The answer of a [`TestApprover`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// Allow every device.
    Allow,
    /// Deny every device.
    Deny,
    /// Never answer, so the engine times out.
    Never,
}

/// A [`PairingApprover`] with a fixed answer. It records each request.
#[derive(Debug)]
pub struct TestApprover {
    answer: Mutex<Approval>,
    requests: Mutex<Vec<PairRequestInfo>>,
}

impl TestApprover {
    /// Makes an approver with an answer.
    pub fn new(answer: Approval) -> Arc<TestApprover> {
        Arc::new(TestApprover {
            answer: Mutex::new(answer),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// Changes the answer.
    pub fn set_answer(&self, answer: Approval) {
        *lock(&self.answer) = answer;
    }

    /// Returns the requests that the engine asked about.
    pub fn requests(&self) -> Vec<PairRequestInfo> {
        lock(&self.requests).clone()
    }
}

impl PairingApprover for TestApprover {
    fn approve(&self, request: PairRequestInfo) -> BoxFuture<'_, bool> {
        lock(&self.requests).push(request);
        let answer = *lock(&self.answer);
        Box::pin(async move {
            match answer {
                Approval::Allow => true,
                Approval::Deny => false,
                Approval::Never => std::future::pending().await,
            }
        })
    }
}
