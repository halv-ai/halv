use super::{
    adapter::{InteractiveTranscriptDiscovery, InteractiveTranscriptQuery},
    claude, codex, AgentEvent, AgentKind, AgentSessionManager, EmbeddedTurnOutcome, SessionMode,
};
use crate::storage::Storage;
use chrono::NaiveDate;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use tauri::AppHandle;
use tauri::Manager;

const TRANSCRIPT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// While a Codex pane runs a turn or waits for the user, the watcher reads
/// the rollouts this often. Turn ends and answers then show sooner.
const ACTIVE_CODEX_POLL_INTERVAL: Duration = Duration::from_millis(500);
const TRANSCRIPT_DISCOVERY_INTERVAL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TranscriptKind {
    Claude,
    Codex,
    Gemini,
}

impl TranscriptKind {
    fn from_agent_kind(agent_kind: AgentKind) -> Option<Self> {
        match agent_kind {
            AgentKind::Claude => Some(Self::Claude),
            AgentKind::Codex => Some(Self::Codex),
            AgentKind::Gemini => Some(Self::Gemini),
            AgentKind::Antigravity | AgentKind::Grok | AgentKind::Kimi | AgentKind::Zai => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GroupKey {
    kind: TranscriptKind,
    cwd: PathBuf,
}

#[derive(Debug, Clone)]
struct TrackedSession {
    id: String,
    group: GroupKey,
    cli_session_id: Option<String>,
    started_at: SystemTime,
}

#[derive(Debug, Clone, Copy)]
struct TranscriptTimes {
    created_at: SystemTime,
    modified_at: SystemTime,
}

#[derive(Debug)]
struct Transcript {
    id: String,
    path: PathBuf,
    times: TranscriptTimes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Adoption {
    session_id: String,
    cli_session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AmbiguousGroup {
    candidate_count: usize,
    silent_session_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexTurnEvent {
    Started {
        turn_id: Option<String>,
    },
    Finished {
        turn_id: Option<String>,
        outcome: EmbeddedTurnOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EmbeddedTurnUpdate {
    session_id: String,
    cli_session_id: String,
    event: CodexTurnEvent,
    /// For the first read of a rollout: the rollout time (Unix ms) of its
    /// last turn event. The agent status treats an event from before the
    /// pane was tracked as history.
    first_read_at: Option<u64>,
}

#[derive(Debug, Default)]
struct CodexTranscriptCursor {
    offset: u64,
    initialized: bool,
    /// Unix milliseconds of the last tool result in the last read.
    tool_output_at: Option<u64>,
    /// Unix milliseconds of the last turn event in the last read.
    turn_event_at: Option<u64>,
}

/// A tool result in the rollout of a running Codex pane. After an approval
/// or a question, it means that the user answered.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexToolOutput {
    session_id: String,
    at: u64,
}

#[derive(Debug, Default)]
struct PollResult {
    adoptions: Vec<Adoption>,
    ambiguous_groups: Vec<AmbiguousGroup>,
    memory_sources: Vec<super::memories::MemorySource>,
    embedded_turn_updates: Vec<EmbeddedTurnUpdate>,
    codex_tool_outputs: Vec<CodexToolOutput>,
}

#[derive(Debug, Default)]
struct TranscriptRoots {
    claude_projects: Option<PathBuf>,
    codex_sessions: Option<PathBuf>,
}

impl TranscriptRoots {
    fn system() -> Self {
        Self {
            claude_projects: claude::transcript_projects_directory(),
            codex_sessions: codex::sessions_root(),
        }
    }
}

#[derive(Debug, Default)]
struct TranscriptWatcherState {
    previous_modified: HashMap<PathBuf, SystemTime>,
    initialized_groups: HashSet<GroupKey>,
    codex_cursors: HashMap<PathBuf, CodexTranscriptCursor>,
    codex_paths_by_cli_session_id: HashMap<String, PathBuf>,
    tracked_session_ids: HashSet<String>,
}

impl TranscriptWatcherState {
    fn tracked_session_membership_changed(&mut self, sessions: &[TrackedSession]) -> bool {
        let next = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<HashSet<_>>();
        if next == self.tracked_session_ids {
            return false;
        }
        self.tracked_session_ids = next;
        true
    }

    fn poll<F>(
        &mut self,
        sessions: &[TrackedSession],
        roots: &TranscriptRoots,
        today: NaiveDate,
        read_times: &F,
    ) -> PollResult
    where
        F: Fn(&Path) -> Option<TranscriptTimes>,
    {
        let used_ids = sessions
            .iter()
            .filter_map(|session| session.cli_session_id.as_deref())
            .filter(|session_id| !session_id.is_empty())
            .collect::<HashSet<_>>();
        let mut groups = HashMap::<GroupKey, Vec<&TrackedSession>>::new();
        for session in sessions {
            groups
                .entry(session.group.clone())
                .or_default()
                .push(session);
        }

        let mut result = PollResult::default();
        let mut next_modified = HashMap::new();
        let mut next_initialized = HashSet::new();
        let mut active_codex_paths = HashSet::new();
        let mut active_codex_ids = HashSet::new();

        for (group, group_sessions) in groups {
            let Some(started_at) = group_sessions
                .iter()
                .map(|session| session.started_at)
                .min()
            else {
                continue;
            };
            let Some(transcripts) = scan_group(&group, roots, today, started_at, read_times) else {
                continue;
            };
            let was_initialized = self.initialized_groups.contains(&group);
            next_initialized.insert(group.clone());
            for transcript in &transcripts {
                next_modified.insert(transcript.path.clone(), transcript.times.modified_at);
            }
            let memory_kind = match group.kind {
                TranscriptKind::Codex => Some(AgentKind::Codex),
                TranscriptKind::Claude => Some(AgentKind::Claude),
                TranscriptKind::Gemini => None,
            };
            if let Some(memory_kind) = memory_kind {
                for session in &group_sessions {
                    if let Some(transcript) = session
                        .cli_session_id
                        .as_ref()
                        .and_then(|id| transcripts.iter().find(|transcript| &transcript.id == id))
                    {
                        result.memory_sources.push(super::memories::MemorySource {
                            session_id: session.id.clone(),
                            kind: memory_kind,
                            path: transcript.path.clone(),
                        });
                    }
                }
            }
            let mut adopted = None;
            if was_initialized || group.kind == TranscriptKind::Gemini {
                let silent_sessions = group_sessions
                    .iter()
                    .copied()
                    .filter(|session| {
                        let Some(current_id) = session.cli_session_id.as_deref() else {
                            return group.kind == TranscriptKind::Gemini;
                        };
                        let current = transcripts
                            .iter()
                            .find(|transcript| transcript.id == current_id);
                        match current {
                            Some(transcript) => self
                                .previous_modified
                                .get(&transcript.path)
                                .is_some_and(|previous| transcript.times.modified_at <= *previous),
                            // An old Codex rollout can fall outside the two-day scan.
                            // Its unchanged identifier still marks this session as silent.
                            None => true,
                        }
                    })
                    .collect::<Vec<_>>();

                let candidate_sessions = if silent_sessions.len() == 1 {
                    silent_sessions.clone()
                } else {
                    group_sessions.clone()
                };
                let candidates = transcripts
                    .iter()
                    .filter(|transcript| !used_ids.contains(transcript.id.as_str()))
                    .filter(|transcript| {
                        self.previous_modified
                            .get(&transcript.path)
                            .is_none_or(|previous| transcript.times.modified_at > *previous)
                    })
                    .filter(|transcript| {
                        candidate_sessions.iter().any(|session| {
                            if group.kind == TranscriptKind::Gemini {
                                transcript.times.created_at >= session.started_at
                            } else {
                                transcript.times.created_at > session.started_at
                            }
                        })
                    })
                    .collect::<Vec<_>>();

                if candidates.len() == 1 && silent_sessions.len() == 1 {
                    adopted = Some(Adoption {
                        session_id: silent_sessions[0].id.clone(),
                        cli_session_id: candidates[0].id.clone(),
                    });
                    result
                        .adoptions
                        .push(adopted.as_ref().expect("adoption was set").clone());
                } else if !candidates.is_empty() {
                    result.ambiguous_groups.push(AmbiguousGroup {
                        candidate_count: candidates.len(),
                        silent_session_count: silent_sessions.len(),
                    });
                }
            }

            if group.kind != TranscriptKind::Codex {
                continue;
            }
            for session in group_sessions {
                let adopted_id = adopted
                    .as_ref()
                    .filter(|adoption| adoption.session_id == session.id)
                    .map(|adoption| adoption.cli_session_id.as_str());
                let Some(cli_session_id) = adopted_id.or(session.cli_session_id.as_deref()) else {
                    continue;
                };
                let Some(transcript) = transcripts
                    .iter()
                    .find(|transcript| transcript.id == cli_session_id)
                else {
                    continue;
                };
                active_codex_paths.insert(transcript.path.clone());
                active_codex_ids.insert(cli_session_id.to_string());
                self.codex_paths_by_cli_session_id
                    .insert(cli_session_id.to_string(), transcript.path.clone());
                let Some((was_cursor_initialized, events)) = read_codex_turn_events(
                    &transcript.path,
                    self.codex_cursors
                        .entry(transcript.path.clone())
                        .or_default(),
                ) else {
                    continue;
                };
                let current_process_transcript =
                    adopted_id.is_some() || transcript.times.created_at >= session.started_at;
                push_tool_output(
                    &mut result,
                    &self.codex_cursors,
                    &transcript.path,
                    &session.id,
                    was_cursor_initialized,
                );
                let first_read_at = first_read_turn_at(
                    &self.codex_cursors,
                    &transcript.path,
                    was_cursor_initialized,
                );
                let events = if was_cursor_initialized {
                    events
                } else {
                    initial_codex_turn_events(events, current_process_transcript)
                };
                result
                    .embedded_turn_updates
                    .extend(events.into_iter().map(|event| EmbeddedTurnUpdate {
                        session_id: session.id.clone(),
                        cli_session_id: cli_session_id.to_string(),
                        event,
                        first_read_at,
                    }));
            }
        }

        self.previous_modified = next_modified;
        self.initialized_groups = next_initialized;
        self.codex_cursors
            .retain(|path, _| active_codex_paths.contains(path));
        self.codex_paths_by_cli_session_id
            .retain(|cli_session_id, _| active_codex_ids.contains(cli_session_id));
        result
    }

    fn poll_cached_codex_activity(&mut self, sessions: &[TrackedSession]) -> PollResult {
        let mut result = PollResult::default();
        let mut active_codex_paths = HashSet::new();
        let mut active_codex_ids = HashSet::new();

        for session in sessions
            .iter()
            .filter(|session| session.group.kind == TranscriptKind::Codex)
        {
            let Some(cli_session_id) = session.cli_session_id.as_deref() else {
                continue;
            };
            let Some(path) = self
                .codex_paths_by_cli_session_id
                .get(cli_session_id)
                .cloned()
            else {
                continue;
            };
            active_codex_paths.insert(path.clone());
            active_codex_ids.insert(cli_session_id.to_string());
            let Some((was_cursor_initialized, events)) =
                read_codex_turn_events(&path, self.codex_cursors.entry(path.clone()).or_default())
            else {
                continue;
            };
            push_tool_output(
                &mut result,
                &self.codex_cursors,
                &path,
                &session.id,
                was_cursor_initialized,
            );
            let first_read_at =
                first_read_turn_at(&self.codex_cursors, &path, was_cursor_initialized);
            let events = if was_cursor_initialized {
                events
            } else {
                initial_codex_turn_events(events, false)
            };
            result
                .embedded_turn_updates
                .extend(events.into_iter().map(|event| EmbeddedTurnUpdate {
                    session_id: session.id.clone(),
                    cli_session_id: cli_session_id.to_string(),
                    event,
                    first_read_at,
                }));
        }

        self.codex_cursors
            .retain(|path, _| active_codex_paths.contains(path));
        self.codex_paths_by_cli_session_id
            .retain(|cli_session_id, _| active_codex_ids.contains(cli_session_id));
        result
    }
}

pub(crate) fn spawn_transcript_watcher(app: AppHandle, manager: AgentSessionManager) {
    let state = Arc::new(Mutex::new(TranscriptWatcherState::default()));
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(ACTIVE_CODEX_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_discovery = None;
        let mut last_poll: Option<Instant> = None;
        loop {
            interval.tick().await;
            let now = Instant::now();
            let due = last_poll
                .is_none_or(|last| now.saturating_duration_since(last) >= TRANSCRIPT_POLL_INTERVAL)
                || crate::agent_status::codex_active(&app);
            if !due {
                continue;
            }
            last_poll = Some(now);
            let discover = last_discovery.is_none_or(|last| {
                now.saturating_duration_since(last) >= TRANSCRIPT_DISCOVERY_INTERVAL
            });
            if discover {
                last_discovery = Some(now);
            }
            let app = app.clone();
            let manager = manager.clone();
            let state = state.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || {
                poll_transcripts(&app, &manager, &state, discover)
            })
            .await;
        }
    });
}

fn poll_transcripts(
    app: &AppHandle,
    manager: &AgentSessionManager,
    state: &Mutex<TranscriptWatcherState>,
    discover: bool,
) {
    let sessions = tracked_sessions(manager);
    let result = {
        let Ok(mut state) = state.lock() else {
            return;
        };
        let membership_changed = state.tracked_session_membership_changed(&sessions);
        let discover = discover || membership_changed;
        if discover {
            state.poll(
                &sessions,
                &TranscriptRoots::system(),
                chrono::Local::now().date_naive(),
                &filesystem_times,
            )
        } else {
            state.poll_cached_codex_activity(&sessions)
        }
    };
    for ambiguity in result.ambiguous_groups {
        eprintln!(
            "[debug] transcript watcher declined an ambiguous group: {} candidates, {} silent sessions",
            ambiguity.candidate_count, ambiguity.silent_session_count
        );
    }
    for adoption in result.adoptions {
        adopt_cli_session_id(app, manager, &adoption);
    }
    for source in result.memory_sources {
        super::memories::schedule(app, source);
    }
    for update in result.embedded_turn_updates {
        emit_embedded_turn_update(app, manager, &update);
    }
    for output in result.codex_tool_outputs {
        crate::agent_status::note_rollout_activity(app, &output.session_id, output.at);
    }
}

/// The rollout time of the last turn event of a first read, if any.
fn first_read_turn_at(
    cursors: &HashMap<PathBuf, CodexTranscriptCursor>,
    path: &Path,
    was_cursor_initialized: bool,
) -> Option<u64> {
    if was_cursor_initialized {
        return None;
    }
    cursors.get(path).and_then(|cursor| cursor.turn_event_at)
}

/// Reports the last tool result of a live read. The first read of a
/// rollout is history.
fn push_tool_output(
    result: &mut PollResult,
    cursors: &HashMap<PathBuf, CodexTranscriptCursor>,
    path: &Path,
    session_id: &str,
    was_cursor_initialized: bool,
) {
    let Some(at) = cursors
        .get(path)
        .and_then(|cursor| cursor.tool_output_at)
        .filter(|_| was_cursor_initialized)
    else {
        return;
    };
    result.codex_tool_outputs.push(CodexToolOutput {
        session_id: session_id.to_string(),
        at,
    });
}

pub(super) fn recent_memory_sources(
    app: &AppHandle,
) -> Result<Vec<super::memories::MemorySource>, String> {
    let storage = app.state::<Storage>();
    let rows = storage
        .recent_memory_agent_sessions()
        .map_err(|e| e.to_string())?;
    let mut sessions = Vec::new();
    for row in rows {
        let kind = match row.kind.as_str() {
            "codex" => TranscriptKind::Codex,
            "claude" => TranscriptKind::Claude,
            _ => continue,
        };
        let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&row.created_at) else {
            continue;
        };
        sessions.push(TrackedSession {
            id: row.id,
            group: GroupKey {
                kind,
                cwd: PathBuf::from(row.cwd),
            },
            cli_session_id: Some(row.cli_session_id),
            started_at: started_at.with_timezone(&chrono::Utc).into(),
        });
    }
    let mut state = TranscriptWatcherState::default();
    let mut sources = state
        .poll(
            &sessions,
            &TranscriptRoots::system(),
            chrono::Local::now().date_naive(),
            &filesystem_times,
        )
        .memory_sources;
    sources.sort_by_key(|source| {
        sessions
            .iter()
            .position(|session| session.id == source.session_id)
            .unwrap_or(usize::MAX)
    });
    sources.truncate(8);
    Ok(sources)
}

fn tracked_sessions(manager: &AgentSessionManager) -> Vec<TrackedSession> {
    let Ok(sessions) = manager.sessions.lock() else {
        return Vec::new();
    };
    sessions
        .values()
        .filter_map(|managed| {
            let record = managed.record.lock().ok()?;
            if record.status != "running"
                || record.mode != SessionMode::TerminalEmbedded
                || record.cwd.trim().is_empty()
            {
                return None;
            }
            let kind = TranscriptKind::from_agent_kind(record.agent_kind)?;
            let started_at = chrono::DateTime::parse_from_rfc3339(&record.created_at).ok()?;
            let started_at: SystemTime = started_at.with_timezone(&chrono::Utc).into();
            Some(TrackedSession {
                id: record.id.clone(),
                group: GroupKey {
                    kind,
                    cwd: PathBuf::from(&record.cwd),
                },
                cli_session_id: record.cli_session_id.clone(),
                started_at,
            })
        })
        .collect()
}

fn adopt_cli_session_id(app: &AppHandle, manager: &AgentSessionManager, adoption: &Adoption) {
    let claimed = {
        let Ok(sessions) = manager.sessions.lock() else {
            return;
        };
        let candidate_is_used = sessions.values().any(|managed| {
            let Ok(record) = managed.record.lock() else {
                return false;
            };
            record.status == "running"
                && record.mode == SessionMode::TerminalEmbedded
                && !record.cwd.trim().is_empty()
                && TranscriptKind::from_agent_kind(record.agent_kind).is_some()
                && record.cli_session_id.as_deref() == Some(adoption.cli_session_id.as_str())
        });
        if candidate_is_used {
            return;
        }
        let Some(managed) = sessions.get(&adoption.session_id).cloned() else {
            return;
        };
        let can_adopt = managed.record.lock().is_ok_and(|record| {
            record.status == "running"
                && record.mode == SessionMode::TerminalEmbedded
                && !record.cwd.trim().is_empty()
                && TranscriptKind::from_agent_kind(record.agent_kind).is_some()
        });
        if !can_adopt {
            return;
        }
        managed.set_cli_session_id(&adoption.cli_session_id);
        managed
    };

    claimed.persist(app);
    let model = claimed.snapshot().model.unwrap_or_default();
    claimed.emit(
        app,
        AgentEvent::SessionInit {
            cli_session_id: adoption.cli_session_id.clone(),
            model,
        },
    );
}

fn emit_embedded_turn_update(
    app: &AppHandle,
    manager: &AgentSessionManager,
    update: &EmbeddedTurnUpdate,
) {
    let managed = {
        let Ok(sessions) = manager.sessions.lock() else {
            return;
        };
        let Some(managed) = sessions.get(&update.session_id).cloned() else {
            return;
        };
        let matches_current_process = managed.record.lock().is_ok_and(|record| {
            record.status == "running"
                && record.mode == SessionMode::TerminalEmbedded
                && record.agent_kind == AgentKind::Codex
                && record.cli_session_id.as_deref() == Some(update.cli_session_id.as_str())
        });
        if !matches_current_process {
            return;
        }
        managed
    };

    let event = match &update.event {
        CodexTurnEvent::Started { turn_id } => AgentEvent::EmbeddedTurnStarted {
            turn_id: turn_id.clone(),
        },
        CodexTurnEvent::Finished { turn_id, outcome } => AgentEvent::EmbeddedTurnFinished {
            turn_id: turn_id.clone(),
            outcome: *outcome,
        },
    };
    crate::agent_status::note_rollout_turn(
        app,
        &update.session_id,
        match &update.event {
            CodexTurnEvent::Started { .. } => crate::agent_status::RolloutTurn::Started,
            CodexTurnEvent::Finished { outcome, .. } => {
                crate::agent_status::RolloutTurn::Finished {
                    aborted: *outcome == EmbeddedTurnOutcome::Aborted,
                }
            }
        },
        update.first_read_at,
    );
    // This event describes work inside the still-running TUI process. Bypass
    // handle_event so persisted process status remains `running` until exit.
    managed.emit(app, event);
}

fn read_codex_turn_events(
    path: &Path,
    cursor: &mut CodexTranscriptCursor,
) -> Option<(bool, Vec<CodexTurnEvent>)> {
    let mut file = fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len < cursor.offset {
        cursor.offset = 0;
        cursor.initialized = false;
    }
    file.seek(SeekFrom::Start(cursor.offset)).ok()?;
    let was_initialized = cursor.initialized;
    cursor.tool_output_at = None;
    cursor.turn_event_at = None;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).ok()?;
        if bytes_read == 0 {
            break;
        }
        // Writers append one complete JSON object per line. Retain the cursor
        // before a partial tail so the next poll can parse it after completion.
        if !line.ends_with('\n') {
            break;
        }
        cursor.offset = cursor.offset.saturating_add(bytes_read as u64);
        let (event, tool_output_at, line_at) = parse_codex_line(&line);
        if let Some(event) = event {
            events.push(event);
            cursor.turn_event_at = line_at;
        }
        if tool_output_at.is_some() {
            cursor.tool_output_at = tool_output_at;
        }
    }
    cursor.initialized = true;
    Some((was_initialized, events))
}

#[cfg(test)]
fn parse_codex_turn_event(line: &str) -> Option<CodexTurnEvent> {
    parse_codex_line(line).0
}

/// The turn event of one rollout line, the time of a tool result, and the
/// time of the line.
fn parse_codex_line(line: &str) -> (Option<CodexTurnEvent>, Option<u64>, Option<u64>) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return (None, None, None);
    };
    let at = codex_line_at(&value);
    let tool_output_at = at.filter(|_| codex_tool_output(&value));
    (codex_turn_event(&value), tool_output_at, at)
}

/// Unix milliseconds of a rollout line.
fn codex_line_at(value: &Value) -> Option<u64> {
    let timestamp = value.get("timestamp")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .and_then(|time| u64::try_from(time.timestamp_millis()).ok())
}

/// True for a tool result line, such as the output of a command that the
/// user approved.
fn codex_tool_output(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("response_item")
        && value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.ends_with("_call_output"))
}

fn codex_turn_event(value: &Value) -> Option<CodexTurnEvent> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    match payload.get("type").and_then(Value::as_str)? {
        "task_started" => Some(CodexTurnEvent::Started { turn_id }),
        "task_complete" => Some(CodexTurnEvent::Finished {
            turn_id,
            outcome: EmbeddedTurnOutcome::Completed,
        }),
        "turn_aborted" => Some(CodexTurnEvent::Finished {
            turn_id,
            outcome: EmbeddedTurnOutcome::Aborted,
        }),
        _ => None,
    }
}

fn initial_codex_turn_events(
    events: Vec<CodexTurnEvent>,
    current_process_transcript: bool,
) -> Vec<CodexTurnEvent> {
    let Some(last) = events.last() else {
        return Vec::new();
    };
    if matches!(last, CodexTurnEvent::Started { .. }) {
        return vec![last.clone()];
    }
    if !current_process_transcript {
        // A completed turn already present when HALV starts is history. Do not
        // turn it into a fresh unread response or onboarding completion.
        return Vec::new();
    }
    let CodexTurnEvent::Finished {
        turn_id: finished_turn_id,
        ..
    } = last
    else {
        return Vec::new();
    };
    let started = match finished_turn_id {
        Some(finished_turn_id) => events.iter().rev().skip(1).find(|event| {
            matches!(
                event,
                CodexTurnEvent::Started { turn_id: Some(turn_id) }
                    if turn_id == finished_turn_id
            )
        }),
        None => events
            .get(events.len().saturating_sub(2))
            .filter(|event| matches!(event, CodexTurnEvent::Started { .. })),
    };
    match started {
        Some(started) => vec![started.clone(), last.clone()],
        None => Vec::new(),
    }
}

fn scan_group<F>(
    group: &GroupKey,
    roots: &TranscriptRoots,
    today: NaiveDate,
    started_at: SystemTime,
    read_times: &F,
) -> Option<Vec<Transcript>>
where
    F: Fn(&Path) -> Option<TranscriptTimes>,
{
    match group.kind {
        TranscriptKind::Claude => scan_claude(group, roots, read_times),
        TranscriptKind::Codex => scan_codex(group, roots, today, read_times),
        TranscriptKind::Gemini => scan_gemini(group, started_at),
    }
}

fn scan_gemini(group: &GroupKey, started_at: SystemTime) -> Option<Vec<Transcript>> {
    let discovery = super::adapter_for(AgentKind::Gemini)
        .discover_interactive_transcripts(InteractiveTranscriptQuery {
            cwd: &group.cwd,
            started_at,
        })
        .ok()?;
    match discovery {
        InteractiveTranscriptDiscovery::Found(transcripts) => Some(
            transcripts
                .into_iter()
                .map(|transcript| Transcript {
                    id: transcript.cli_session_id,
                    path: transcript.path,
                    times: TranscriptTimes {
                        created_at: transcript.started_at,
                        modified_at: transcript.modified_at,
                    },
                })
                .collect(),
        ),
        InteractiveTranscriptDiscovery::Unsupported(_) => None,
    }
}

fn scan_claude<F>(
    group: &GroupKey,
    roots: &TranscriptRoots,
    read_times: &F,
) -> Option<Vec<Transcript>>
where
    F: Fn(&Path) -> Option<TranscriptTimes>,
{
    let projects_directory = roots.claude_projects.as_deref()?;
    let project_directory = claude::transcript_project_directory_in(projects_directory, &group.cwd);
    let entries = fs::read_dir(project_directory).ok()?;
    let mut transcripts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(times) = read_times(&path) else {
            continue;
        };
        transcripts.push(Transcript {
            id: id.to_string(),
            path,
            times,
        });
    }
    Some(transcripts)
}

fn scan_codex<F>(
    group: &GroupKey,
    roots: &TranscriptRoots,
    today: NaiveDate,
    read_times: &F,
) -> Option<Vec<Transcript>>
where
    F: Fn(&Path) -> Option<TranscriptTimes>,
{
    let sessions_directory = roots.codex_sessions.as_deref()?;
    let mut transcripts = Vec::new();
    for date in [Some(today), today.pred_opt()].into_iter().flatten() {
        let directory = sessions_directory.join(date.format("%Y/%m/%d").to_string());
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some((_, id)) = codex::rollout_created_at_and_session_id(&path) else {
                continue;
            };
            if !strict_rollout_matches_cwd(&path, &group.cwd) {
                continue;
            }
            let Some(times) = read_times(&path) else {
                continue;
            };
            transcripts.push(Transcript { id, path, times });
        }
    }
    Some(transcripts)
}

fn strict_rollout_matches_cwd(path: &Path, cwd: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut first_line = String::new();
    let Ok(bytes_read) = BufReader::new(file).read_line(&mut first_line) else {
        return false;
    };
    if bytes_read == 0 {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&first_line) else {
        return false;
    };
    value
        .get("payload")
        .and_then(|payload| payload.get("cwd"))
        .and_then(Value::as_str)
        .is_some_and(|recorded| Path::new(recorded) == cwd)
}

fn filesystem_times(path: &Path) -> Option<TranscriptTimes> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(TranscriptTimes {
        created_at: metadata.created().ok()?,
        modified_at: metadata.modified().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write as _, time::UNIX_EPOCH};

    const TODAY: NaiveDate = match NaiveDate::from_ymd_opt(2026, 8, 22) {
        Some(date) => date,
        None => panic!("test date must be valid"),
    };

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-transcript-watcher-{name}-{:032x}",
                rand::random::<u128>()
            ));
            fs::create_dir_all(&path).expect("test directory should be created");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Default)]
    struct TestTimes {
        values: HashMap<PathBuf, TranscriptTimes>,
    }

    impl TestTimes {
        fn set(&mut self, path: &Path, created_at: u64, modified_at: u64) {
            self.values.insert(
                path.to_path_buf(),
                TranscriptTimes {
                    created_at: at(created_at),
                    modified_at: at(modified_at),
                },
            );
        }

        fn read(&self, path: &Path) -> Option<TranscriptTimes> {
            self.values.get(path).copied()
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn roots(directory: &TestDirectory) -> TranscriptRoots {
        TranscriptRoots {
            claude_projects: Some(directory.path.join("claude/projects")),
            codex_sessions: Some(directory.path.join("codex/sessions")),
        }
    }

    fn claude_session(
        id: &str,
        cwd: &Path,
        cli_session_id: &str,
        started_at: u64,
    ) -> TrackedSession {
        TrackedSession {
            id: id.to_string(),
            group: GroupKey {
                kind: TranscriptKind::Claude,
                cwd: cwd.to_path_buf(),
            },
            cli_session_id: Some(cli_session_id.to_string()),
            started_at: at(started_at),
        }
    }

    fn codex_session(
        id: &str,
        cwd: &Path,
        cli_session_id: &str,
        started_at: u64,
    ) -> TrackedSession {
        TrackedSession {
            id: id.to_string(),
            group: GroupKey {
                kind: TranscriptKind::Codex,
                cwd: cwd.to_path_buf(),
            },
            cli_session_id: Some(cli_session_id.to_string()),
            started_at: at(started_at),
        }
    }

    fn gemini_session(
        id: &str,
        cwd: &Path,
        cli_session_id: Option<&str>,
        started_at: u64,
    ) -> TrackedSession {
        TrackedSession {
            id: id.to_string(),
            group: GroupKey {
                kind: TranscriptKind::Gemini,
                cwd: cwd.to_path_buf(),
            },
            cli_session_id: cli_session_id.map(str::to_string),
            started_at: at(started_at),
        }
    }

    fn write_gemini_transcript(runtime: &Path, cwd: &Path, session_id: &str) -> PathBuf {
        let chats = runtime.join("tmp/workspace/chats");
        fs::create_dir_all(&chats).expect("Gemini chat directory should be created");
        let path = chats.join(format!("session-110-{}.jsonl", &session_id[..8]));
        let metadata = serde_json::json!({
            "sessionId": session_id,
            "projectHash": super::super::google::gemini_project_hash(cwd)
                .expect("cwd should be UTF-8"),
            "startTime": "1970-01-01T00:01:50Z",
            "lastUpdated": "1970-01-01T00:02:00Z",
        });
        fs::write(&path, format!("{metadata}\n")).expect("Gemini transcript should be written");
        path
    }

    fn write_claude_transcript(roots: &TranscriptRoots, cwd: &Path, session_id: &str) -> PathBuf {
        let projects = roots
            .claude_projects
            .as_deref()
            .expect("Claude projects directory should exist");
        let project = claude::transcript_project_directory_in(projects, cwd);
        fs::create_dir_all(&project).expect("Claude project directory should be created");
        let path = project.join(format!("{session_id}.jsonl"));
        fs::write(&path, b"{}\n").expect("Claude transcript should be written");
        path
    }

    fn write_codex_transcript(
        roots: &TranscriptRoots,
        cwd: &Path,
        session_id: &str,
        events: &str,
    ) -> PathBuf {
        let sessions = roots
            .codex_sessions
            .as_deref()
            .expect("Codex sessions directory should exist")
            .join(TODAY.format("%Y/%m/%d").to_string());
        fs::create_dir_all(&sessions).expect("Codex date directory should be created");
        let path = sessions.join(format!("rollout-2026-08-22T10-00-00-{session_id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n{events}",
                cwd.display()
            ),
        )
        .expect("Codex transcript should be written");
        path
    }

    #[test]
    fn session_membership_change_requests_one_immediate_discovery() {
        let cwd = Path::new("/work/project");
        let first = codex_session("pane-1", cwd, "codex-1", 100);
        let second = codex_session("pane-2", cwd, "codex-2", 101);
        let mut watcher = TranscriptWatcherState::default();

        assert!(watcher.tracked_session_membership_changed(std::slice::from_ref(&first)));
        assert!(!watcher.tracked_session_membership_changed(std::slice::from_ref(&first)));
        assert!(watcher.tracked_session_membership_changed(&[first.clone(), second]));
        assert!(watcher.tracked_session_membership_changed(&[first]));
    }

    #[test]
    fn gemini_adopts_a_full_resume_id_for_an_id_less_pane_on_first_poll() {
        let directory = TestDirectory::new("gemini-id-less");
        let runtime = directory.path.join(".gemini");
        let cwd = Path::new("/work/project");
        let cli_session_id = "11111111-1111-4111-8111-111111111111";
        write_gemini_transcript(&runtime, cwd, cli_session_id);
        let session = gemini_session("pane-1", cwd, None, 100);
        let roots = roots(&directory);
        let mut watcher = TranscriptWatcherState::default();

        let result = super::super::google::with_test_gemini_runtime_directory(runtime, || {
            watcher.poll(&[session], &roots, TODAY, &filesystem_times)
        });

        assert_eq!(
            result.adoptions,
            [Adoption {
                session_id: "pane-1".to_string(),
                cli_session_id: cli_session_id.to_string(),
            }]
        );
        assert!(result.memory_sources.is_empty());
    }

    #[test]
    fn gemini_replaces_a_stale_resume_id_on_first_poll() {
        let directory = TestDirectory::new("gemini-stale-resume");
        let runtime = directory.path.join(".gemini");
        let cwd = Path::new("/work/project");
        let cli_session_id = "22222222-2222-4222-8222-222222222222";
        write_gemini_transcript(&runtime, cwd, cli_session_id);
        let session = gemini_session("pane-1", cwd, Some("stale-session"), 100);
        let roots = roots(&directory);
        let mut watcher = TranscriptWatcherState::default();

        let result = super::super::google::with_test_gemini_runtime_directory(runtime, || {
            watcher.poll(&[session], &roots, TODAY, &filesystem_times)
        });

        assert_eq!(
            result.adoptions,
            [Adoption {
                session_id: "pane-1".to_string(),
                cli_session_id: cli_session_id.to_string(),
            }]
        );
    }

    #[test]
    fn adopts_one_advancing_transcript_for_one_silent_session() {
        let directory = TestDirectory::new("one-candidate");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let old = write_claude_transcript(&roots, cwd, "old-session");
        let new = write_claude_transcript(&roots, cwd, "new-session");
        let sessions = vec![claude_session("pane-1", cwd, "old-session", 100)];
        let mut times = TestTimes::default();
        times.set(&old, 90, 120);
        times.set(&new, 110, 130);
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        times.set(&new, 110, 140);
        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert_eq!(
            result.adoptions,
            vec![Adoption {
                session_id: "pane-1".to_string(),
                cli_session_id: "new-session".to_string(),
            }]
        );
    }

    #[test]
    fn declines_two_sessions_and_two_new_transcripts() {
        let directory = TestDirectory::new("ambiguous");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let old_one = write_claude_transcript(&roots, cwd, "old-one");
        let old_two = write_claude_transcript(&roots, cwd, "old-two");
        let new_one = write_claude_transcript(&roots, cwd, "new-one");
        let new_two = write_claude_transcript(&roots, cwd, "new-two");
        let sessions = vec![
            claude_session("pane-1", cwd, "old-one", 100),
            claude_session("pane-2", cwd, "old-two", 100),
        ];
        let mut times = TestTimes::default();
        times.set(&old_one, 90, 120);
        times.set(&old_two, 90, 120);
        times.set(&new_one, 110, 130);
        times.set(&new_two, 111, 131);
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        times.set(&new_one, 110, 140);
        times.set(&new_two, 111, 141);
        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert!(result.adoptions.is_empty());
        assert_eq!(
            result.ambiguous_groups,
            vec![AmbiguousGroup {
                candidate_count: 2,
                silent_session_count: 2,
            }]
        );
    }

    #[test]
    fn rejects_a_transcript_created_before_the_session_started() {
        let directory = TestDirectory::new("old-candidate");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let old = write_claude_transcript(&roots, cwd, "old-session");
        let candidate = write_claude_transcript(&roots, cwd, "candidate");
        let sessions = vec![claude_session("pane-1", cwd, "old-session", 100)];
        let mut times = TestTimes::default();
        times.set(&old, 80, 120);
        times.set(&candidate, 99, 130);
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        times.set(&candidate, 99, 140);
        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert!(result.adoptions.is_empty());
    }

    #[test]
    fn rejects_another_tracked_sessions_current_id() {
        let directory = TestDirectory::new("used-candidate");
        let roots = roots(&directory);
        let first_cwd = Path::new("/work/first");
        let second_cwd = Path::new("/work/second");
        let old = write_claude_transcript(&roots, first_cwd, "old-session");
        let used = write_claude_transcript(&roots, first_cwd, "used-session");
        let sessions = vec![
            claude_session("pane-1", first_cwd, "old-session", 100),
            claude_session("pane-2", second_cwd, "used-session", 100),
        ];
        let mut times = TestTimes::default();
        times.set(&old, 90, 120);
        times.set(&used, 110, 130);
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        times.set(&used, 110, 140);
        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert!(result.adoptions.is_empty());
    }

    #[test]
    fn skips_a_missing_claude_project_directory() {
        let directory = TestDirectory::new("missing-project");
        let roots = roots(&directory);
        let sessions = vec![claude_session(
            "pane-1",
            Path::new("/work/missing"),
            "old-session",
            100,
        )];
        let times = TestTimes::default();
        let mut watcher = TranscriptWatcherState::default();

        let first = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        let second = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert!(first.adoptions.is_empty());
        assert!(second.adoptions.is_empty());
    }

    #[test]
    fn codex_adopts_when_the_old_rollout_is_outside_the_scan_window() {
        let directory = TestDirectory::new("codex-old-rollout");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let sessions_directory = roots
            .codex_sessions
            .as_deref()
            .expect("Codex sessions directory should exist")
            .join(TODAY.format("%Y/%m/%d").to_string());
        fs::create_dir_all(&sessions_directory).expect("Codex date directory should be created");
        let new_id = "11111111-1111-4111-8111-111111111111";
        let rollout =
            sessions_directory.join(format!("rollout-2026-08-22T10-00-00-{new_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                cwd.display()
            ),
        )
        .expect("Codex rollout should be written");
        let sessions = vec![TrackedSession {
            id: "pane-1".to_string(),
            group: GroupKey {
                kind: TranscriptKind::Codex,
                cwd: cwd.to_path_buf(),
            },
            cli_session_id: Some("old-codex-session".to_string()),
            started_at: at(100),
        }];
        let mut times = TestTimes::default();
        times.set(&rollout, 110, 130);
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        times.set(&rollout, 110, 140);
        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        assert_eq!(
            result.adoptions,
            vec![Adoption {
                session_id: "pane-1".to_string(),
                cli_session_id: new_id.to_string(),
            }]
        );
    }

    #[test]
    fn tool_results_in_the_rollout_carry_their_time() {
        assert_eq!(
            parse_codex_line(
                r#"{"timestamp":"2026-10-14T17:49:59.581Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":"ok"}}"#,
            ),
            (None, Some(1_792_000_199_581), Some(1_792_000_199_581))
        );
        assert_eq!(
            parse_codex_line(
                r#"{"timestamp":"2026-10-14T17:49:59.581Z","type":"response_item","payload":{"type":"function_call","name":"exec"}}"#,
            ),
            (None, None, Some(1_792_000_199_581))
        );
        assert_eq!(
            parse_codex_line(
                r#"{"timestamp":"bad","type":"response_item","payload":{"type":"function_call_output"}}"#,
            ),
            (None, None, None)
        );
    }

    #[test]
    fn a_first_read_reports_the_time_of_its_last_turn_event() {
        let path = std::env::temp_dir().join(format!(
            "halv-rollout-first-read-{}-{}.jsonl",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::write(
            &path,
            concat!(
                "{\"timestamp\":\"2026-10-14T17:49:49.845Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"t1\"}}\n",
                "{\"timestamp\":\"2026-10-14T17:49:59.581Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"t1\"}}\n",
            ),
        )
        .unwrap();
        let mut cursors = HashMap::new();
        let (initialized, events) =
            read_codex_turn_events(&path, cursors.entry(path.clone()).or_default()).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            first_read_turn_at(&cursors, &path, initialized),
            Some(1_792_000_199_581)
        );
        // A later read is live: it has no history time.
        let (initialized, _) =
            read_codex_turn_events(&path, cursors.entry(path.clone()).or_default()).unwrap();
        assert_eq!(first_read_turn_at(&cursors, &path, initialized), None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_live_read_reports_its_last_tool_result() {
        let path = std::env::temp_dir().join(format!(
            "halv-rollout-activity-{}-{}.jsonl",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::write(
            &path,
            "{\"timestamp\":\"2026-10-14T17:49:49.845Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
        )
        .unwrap();
        let mut cursors = HashMap::new();
        let mut result = PollResult::default();
        let (initialized, _) =
            read_codex_turn_events(&path, cursors.entry(path.clone()).or_default()).unwrap();
        push_tool_output(&mut result, &cursors, &path, "pane", initialized);
        assert!(result.codex_tool_outputs.is_empty());

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            b"{\"timestamp\":\"2026-10-14T17:49:59.581Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\"}}\n",
        )
        .unwrap();
        let (initialized, _) =
            read_codex_turn_events(&path, cursors.entry(path.clone()).or_default()).unwrap();
        push_tool_output(&mut result, &cursors, &path, "pane", initialized);
        assert_eq!(
            result.codex_tool_outputs,
            [CodexToolOutput {
                session_id: "pane".to_string(),
                at: 1_792_000_199_581,
            }]
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_codex_turn_lifecycle_and_preserves_aborted_outcome() {
        assert_eq!(
            parse_codex_turn_event(
                r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1","started_at":1}}"#,
            ),
            Some(CodexTurnEvent::Started {
                turn_id: Some("turn-1".to_string()),
            })
        );
        assert_eq!(
            parse_codex_turn_event(
                r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","completed_at":2}}"#,
            ),
            Some(CodexTurnEvent::Finished {
                turn_id: Some("turn-1".to_string()),
                outcome: EmbeddedTurnOutcome::Completed,
            })
        );
        assert_eq!(
            parse_codex_turn_event(
                r#"{"type":"event_msg","payload":{"type":"turn_aborted","turn_id":"turn-2","reason":"interrupted"}}"#,
            ),
            Some(CodexTurnEvent::Finished {
                turn_id: Some("turn-2".to_string()),
                outcome: EmbeddedTurnOutcome::Aborted,
            })
        );
        assert_eq!(
            parse_codex_turn_event(
                r#"{"type":"response_item","payload":{"type":"task_complete","turn_id":"turn-1"}}"#,
            ),
            None
        );
        assert_eq!(
            serde_json::to_value(AgentEvent::EmbeddedTurnStarted {
                turn_id: Some("turn-1".to_string()),
            })
            .expect("embedded start should serialize"),
            serde_json::json!({
                "type": "embeddedTurnStarted",
                "turnId": "turn-1",
            })
        );
        assert_eq!(
            serde_json::to_value(AgentEvent::EmbeddedTurnFinished {
                turn_id: Some("turn-1".to_string()),
                outcome: EmbeddedTurnOutcome::Aborted,
            })
            .expect("embedded finish should serialize"),
            serde_json::json!({
                "type": "embeddedTurnFinished",
                "turnId": "turn-1",
                "outcome": "aborted",
            })
        );
    }

    #[test]
    fn incremental_reader_waits_for_a_complete_jsonl_record() {
        let directory = TestDirectory::new("codex-partial-line");
        let path = directory.path.join("rollout.jsonl");
        fs::write(
            &path,
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"",
        )
        .expect("partial Codex transcript should be written");
        let mut cursor = CodexTranscriptCursor::default();

        let (initialized, first) =
            read_codex_turn_events(&path, &mut cursor).expect("transcript should be readable");
        assert!(!initialized);
        assert_eq!(
            first,
            vec![CodexTurnEvent::Started {
                turn_id: Some("turn-1".to_string()),
            }]
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("transcript should reopen");
        writeln!(file, ",\"turn_id\":\"turn-1\"}}}}").expect("partial record should be completed");
        let (initialized, second) =
            read_codex_turn_events(&path, &mut cursor).expect("transcript should be readable");
        assert!(initialized);
        assert_eq!(
            second,
            vec![CodexTurnEvent::Finished {
                turn_id: Some("turn-1".to_string()),
                outcome: EmbeddedTurnOutcome::Completed,
            }]
        );
    }

    #[test]
    fn hydration_does_not_replay_a_historic_completion() {
        let events = vec![
            CodexTurnEvent::Started {
                turn_id: Some("historic-turn".to_string()),
            },
            CodexTurnEvent::Finished {
                turn_id: Some("historic-turn".to_string()),
                outcome: EmbeddedTurnOutcome::Completed,
            },
        ];

        assert!(initial_codex_turn_events(events.clone(), false).is_empty());
        assert_eq!(initial_codex_turn_events(events.clone(), true), events);
        assert_eq!(
            initial_codex_turn_events(
                vec![CodexTurnEvent::Started {
                    turn_id: Some("active-turn".to_string()),
                }],
                false,
            ),
            vec![CodexTurnEvent::Started {
                turn_id: Some("active-turn".to_string()),
            }]
        );
    }

    #[test]
    fn known_new_rollout_replays_a_fast_first_turn() {
        let directory = TestDirectory::new("codex-known-new-fast-turn");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let cli_session_id = "11111111-1111-4111-8111-111111111111";
        let path = write_codex_transcript(
            &roots,
            cwd,
            cli_session_id,
            concat!(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn-1\"}}\n"
            ),
        );
        let session = codex_session("pane-1", cwd, cli_session_id, 100);
        let mut times = TestTimes::default();
        times.set(&path, 100, 101);
        let mut watcher = TranscriptWatcherState::default();

        let result = watcher.poll(&[session], &roots, TODAY, &|path| times.read(path));

        assert_eq!(
            result.embedded_turn_updates,
            vec![
                EmbeddedTurnUpdate {
                    session_id: "pane-1".to_string(),
                    cli_session_id: cli_session_id.to_string(),
                    event: CodexTurnEvent::Started {
                        turn_id: Some("turn-1".to_string()),
                    },
                    first_read_at: None,
                },
                EmbeddedTurnUpdate {
                    session_id: "pane-1".to_string(),
                    cli_session_id: cli_session_id.to_string(),
                    event: CodexTurnEvent::Finished {
                        turn_id: Some("turn-1".to_string()),
                        outcome: EmbeddedTurnOutcome::Completed,
                    },
                    first_read_at: None,
                },
            ]
        );
    }

    #[test]
    fn known_resumed_rollout_drops_its_historic_finish() {
        let directory = TestDirectory::new("codex-known-old-resume");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let cli_session_id = "11111111-1111-4111-8111-111111111111";
        let path = write_codex_transcript(
            &roots,
            cwd,
            cli_session_id,
            concat!(
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"historic-turn\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"historic-turn\"}}\n"
            ),
        );
        let session = codex_session("pane-1", cwd, cli_session_id, 100);
        let mut times = TestTimes::default();
        times.set(&path, 99, 101);
        let mut watcher = TranscriptWatcherState::default();

        let result = watcher.poll(&[session], &roots, TODAY, &|path| times.read(path));

        assert!(result.embedded_turn_updates.is_empty());
    }

    #[test]
    fn cached_activity_poll_reads_only_new_transcript_records() {
        let directory = TestDirectory::new("codex-cached-activity");
        let path = directory.path.join("rollout.jsonl");
        fs::write(
            &path,
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"turn-1\"}}\n",
        )
        .expect("Codex transcript should be written");
        let session = TrackedSession {
            id: "pane-1".to_string(),
            group: GroupKey {
                kind: TranscriptKind::Codex,
                cwd: PathBuf::from("/work/project"),
            },
            cli_session_id: Some("codex-session-1".to_string()),
            started_at: at(100),
        };
        let mut watcher = TranscriptWatcherState::default();
        watcher
            .codex_paths_by_cli_session_id
            .insert("codex-session-1".to_string(), path.clone());

        let started = watcher.poll_cached_codex_activity(std::slice::from_ref(&session));
        assert_eq!(
            started.embedded_turn_updates,
            vec![EmbeddedTurnUpdate {
                session_id: "pane-1".to_string(),
                cli_session_id: "codex-session-1".to_string(),
                event: CodexTurnEvent::Started {
                    turn_id: Some("turn-1".to_string()),
                },
                first_read_at: None,
            }]
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("transcript should reopen");
        file.write_all(
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"turn-1\"}}\n",
        )
        .expect("completion should append");

        let finished = watcher.poll_cached_codex_activity(&[session]);
        assert_eq!(
            finished.embedded_turn_updates,
            vec![EmbeddedTurnUpdate {
                session_id: "pane-1".to_string(),
                cli_session_id: "codex-session-1".to_string(),
                event: CodexTurnEvent::Finished {
                    turn_id: Some("turn-1".to_string()),
                    outcome: EmbeddedTurnOutcome::Completed,
                },
                first_read_at: None,
            }]
        );
    }

    #[test]
    fn adopts_a_new_codex_rollout_on_its_first_observed_poll() {
        let directory = TestDirectory::new("codex-new-rollout");
        let roots = roots(&directory);
        let cwd = Path::new("/work/project");
        let sessions = vec![TrackedSession {
            id: "pane-1".to_string(),
            group: GroupKey {
                kind: TranscriptKind::Codex,
                cwd: cwd.to_path_buf(),
            },
            cli_session_id: Some("old-codex-session".to_string()),
            started_at: at(100),
        }];
        let mut times = TestTimes::default();
        let mut watcher = TranscriptWatcherState::default();

        watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));

        let sessions_directory = roots
            .codex_sessions
            .as_deref()
            .expect("Codex sessions directory should exist")
            .join(TODAY.format("%Y/%m/%d").to_string());
        fs::create_dir_all(&sessions_directory).expect("Codex date directory should be created");
        let new_id = "11111111-1111-4111-8111-111111111111";
        let rollout =
            sessions_directory.join(format!("rollout-2026-08-22T10-00-00-{new_id}.jsonl"));
        fs::write(
            &rollout,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"{}\"}}}}\n",
                cwd.display()
            ),
        )
        .expect("Codex rollout should be written");
        times.set(&rollout, 110, 130);

        let result = watcher.poll(&sessions, &roots, TODAY, &|path| times.read(path));
        assert_eq!(
            result.adoptions,
            vec![Adoption {
                session_id: "pane-1".to_string(),
                cli_session_id: new_id.to_string(),
            }]
        );
    }
}
