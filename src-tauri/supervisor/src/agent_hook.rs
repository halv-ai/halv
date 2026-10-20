//! The `agent-hook` helper.
//!
//! Halv writes a per-session Claude Code settings file. Each hook entry in
//! that file runs `halv-agent-hook agent-hook claude`, a stable copy of this
//! binary. The helper keeps a few small fields of the hook payload, appends
//! them as one line to the journal of the pane, and wakes the app.
//!
//! The helper never decides anything for Claude Code. It prints `{}` (valid
//! hook output with no decision) and exits with status 0 in every path. It
//! keeps short previews only, never full prompts, tool input, or output.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The first argument that selects the helper.
pub const AGENT_HOOK_COMMAND: &str = "agent-hook";
/// The terminal session id of the pane. Halv sets it in the PTY.
pub const SESSION_ID_ENV: &str = "HALV_AGENT_SESSION_ID";
/// The folder of the journals and the wake socket. Halv sets it in the PTY.
pub const STATUS_DIR_ENV: &str = "HALV_AGENT_STATUS_DIR";
/// Claude Code sets this in background job workers. A worker inherits the
/// environment of the pane that started it, so its events are not pane events.
const JOB_DIR_ENV: &str = "CLAUDE_JOB_DIR";
/// The datagram socket of the app in the status folder.
pub const WAKE_SOCKET_NAME: &str = "wake.sock";
pub const JOURNAL_RECORD_VERSION: u32 = 1;

/// The helper keeps at most this much of the hook payload.
pub const MAX_STDIN_BYTES: usize = 4 * 1024 * 1024;
/// The helper stops to wait for the payload after this time.
pub const STDIN_TIME_LIMIT: Duration = Duration::from_secs(3);
/// The length of each text preview in characters.
pub const PREVIEW_CHARS: usize = 160;
/// A journal that grows past this size moves to `<id>.jsonl.1`.
pub const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
/// A helper waits this long for the journal lock. After that it writes
/// without the lock: a hook must not stall the agent.
const JOURNAL_LOCK_TIME_LIMIT: Duration = Duration::from_millis(500);
const MAX_OPTIONS: usize = 8;
const OPTION_CHARS: usize = 80;
const ID_CHARS: usize = 128;
const NAME_CHARS: usize = 64;
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// One hook event in the journal of a pane. Each field except the first
/// five is optional and absent when the payload does not have it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookRecord {
    pub v: u32,
    /// Unix milliseconds when the helper ran.
    pub at: u64,
    /// The terminal session id of the pane.
    pub pane: String,
    /// The agent CLI, for example `claude`.
    pub agent: String,
    /// The hook event name, for example `PermissionRequest`.
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<String>,
    /// Present for events of a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// A preview of `tool_input.command`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// A preview of `tool_input.file_path` or `tool_input.notebook_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// A preview of the first `AskUserQuestion` question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The option labels of that question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// A preview of the notification text or of another tool input field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,
}

/// The pane and folder from the PTY environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookTarget {
    pub session_id: String,
    pub status_dir: PathBuf,
}

impl HookTarget {
    /// Reads the target through `lookup`. Returns `None` when the helper must
    /// do nothing: a variable is missing or not valid, or a background job
    /// worker runs the hook.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<OsString>) -> Option<Self> {
        if lookup(JOB_DIR_ENV).is_some_and(|value| !value.is_empty()) {
            return None;
        }
        let session_id = lookup(SESSION_ID_ENV)?.into_string().ok()?;
        if !is_safe_session_id(&session_id) {
            return None;
        }
        let status_dir = PathBuf::from(lookup(STATUS_DIR_ENV)?);
        if !status_dir.is_absolute() {
            return None;
        }
        Some(Self {
            session_id,
            status_dir,
        })
    }
}

/// True for an id that is safe as a file name. Terminal ids are UUIDs.
pub fn is_safe_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= ID_CHARS
        && session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

/// The journal of one pane.
pub fn journal_path(status_dir: &Path, session_id: &str) -> PathBuf {
    status_dir.join(format!("{session_id}.jsonl"))
}

/// The journal that a rotation moved aside.
pub fn rotated_journal_path(status_dir: &Path, session_id: &str) -> PathBuf {
    status_dir.join(format!("{session_id}.jsonl.1"))
}

/// The lock file of a journal. Helpers that run at the same time take it in
/// turn to check the size, rotate, and append.
pub fn journal_lock_path(status_dir: &Path, session_id: &str) -> PathBuf {
    status_dir.join(format!("{session_id}.jsonl.lock"))
}

/// Runs the helper. `arguments` are the arguments after `agent-hook`.
/// Returns the exit status, which is always 0.
pub fn run_from_env(mut arguments: impl Iterator<Item = OsString>) -> i32 {
    // Valid hook output with no decision. Some hook runners treat empty
    // output from a permission hook as a failure.
    let mut stdout = io::stdout();
    let _ = stdout.write_all(b"{}\n");
    let _ = stdout.flush();

    let (payload, truncated) = read_limited(io::stdin(), MAX_STDIN_BYTES, STDIN_TIME_LIMIT);
    let Some(target) = HookTarget::from_lookup(|name| std::env::var_os(name)) else {
        return 0;
    };
    let agent = agent_name(arguments.next().as_deref());
    let Some(record) = parse_payload(&target.session_id, &agent, now_ms(), &payload, truncated)
    else {
        return 0;
    };
    let Ok(line) = serde_json::to_string(&record) else {
        return 0;
    };
    if append_journal(&target.status_dir, &target.session_id, &line).is_ok() {
        wake(&target.status_dir, &target.session_id);
    }
    0
}

fn agent_name(argument: Option<&OsStr>) -> String {
    argument
        .and_then(OsStr::to_str)
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 16
                && name.chars().all(|character| character.is_ascii_lowercase())
        })
        .unwrap_or("claude")
        .to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// Reads `reader` to its end, for at most `time_limit`. It keeps the first
/// `limit` bytes and drops the rest, so the writer never blocks. The flag is
/// true when bytes were dropped or the time ran out.
pub fn read_limited<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
    time_limit: Duration,
) -> (Vec<u8>, bool) {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    // A thread does the reads. A blocked read cannot hold the helper past
    // the time limit, and the process exit ends the thread.
    let spawned = thread::Builder::new().spawn(move || {
        let mut buffer = vec![0; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if sender.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    if spawned.is_err() {
        return (Vec::new(), true);
    }
    let deadline = Instant::now() + time_limit;
    let mut kept = Vec::new();
    let mut truncated = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(chunk) => {
                let room = limit.saturating_sub(kept.len());
                if chunk.len() > room {
                    truncated = true;
                }
                kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                truncated = true;
                break;
            }
        }
    }
    (kept, truncated)
}

/// Builds the journal record of one payload. Returns `None` for a payload
/// without a valid event name. A cut payload is not valid JSON, so the
/// helper then scans its start for the event and tool names.
pub fn parse_payload(
    pane: &str,
    agent: &str,
    at: u64,
    payload: &[u8],
    truncated: bool,
) -> Option<HookRecord> {
    let mut record = HookRecord {
        v: JOURNAL_RECORD_VERSION,
        at,
        pane: pane.to_string(),
        agent: agent.to_string(),
        ..HookRecord::default()
    };
    match serde_json::from_slice::<Value>(payload) {
        Ok(Value::Object(object)) => fill_from_object(&mut record, &object),
        Ok(_) => return None,
        Err(_) if truncated => fill_from_prefix(&mut record, payload),
        Err(_) => return None,
    }
    is_event_name(&record.event).then_some(record)
}

fn is_event_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_CHARS
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn fill_from_object(record: &mut HookRecord, object: &Map<String, Value>) {
    let text = |key: &str, chars: usize| {
        object
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| preview(value, chars))
    };
    record.event = text("hook_event_name", NAME_CHARS).unwrap_or_default();
    record.cli_session_id = text("session_id", ID_CHARS);
    record.agent_id = text("agent_id", ID_CHARS);
    record.tool = text("tool_name", ID_CHARS);
    record.tool_use_id = text("tool_use_id", ID_CHARS);
    record.notification_type = text("notification_type", NAME_CHARS);
    record.source = text("source", NAME_CHARS);
    record.trigger = text("trigger", NAME_CHARS);
    record.stop_hook_active = object.get("stop_hook_active").and_then(Value::as_bool);
    record.error = match object.get("error") {
        Some(Value::String(error)) => preview(error, PREVIEW_CHARS),
        // Some builds send the error as an object with a type or message.
        Some(Value::Object(error)) => ["type", "message"]
            .iter()
            .find_map(|key| error.get(*key).and_then(Value::as_str))
            .and_then(|error| preview(error, PREVIEW_CHARS)),
        _ => None,
    };
    record.message = text("message", PREVIEW_CHARS);

    let Some(input) = object.get("tool_input").and_then(Value::as_object) else {
        return;
    };
    let input_text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .and_then(|value| preview(value, PREVIEW_CHARS))
    };
    record.command = input_text("command");
    record.file_path = input_text("file_path").or_else(|| input_text("notebook_path"));
    if let Some(question) = input
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
    {
        record.question = question
            .get("question")
            .and_then(Value::as_str)
            .and_then(|text| preview(text, PREVIEW_CHARS));
        let options = question
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| match option {
                        Value::String(label) => Some(label.as_str()),
                        Value::Object(option) => option.get("label").and_then(Value::as_str),
                        _ => None,
                    })
                    .filter_map(|label| preview(label, OPTION_CHARS))
                    .take(MAX_OPTIONS)
                    .collect::<Vec<_>>()
            })
            .filter(|options| !options.is_empty());
        record.options = options;
    }
    if record.message.is_none() && record.command.is_none() && record.file_path.is_none() {
        // A short summary for tools without a command or a file, such as a
        // URL to fetch or a plan to approve.
        record.message = ["url", "description", "plan", "skill", "query", "pattern"]
            .iter()
            .find_map(|key| input_text(key));
    }
}

/// Finds the string value of a few top-level keys in the start of a cut
/// payload. It accepts only simple strings with no escapes.
fn fill_from_prefix(record: &mut HookRecord, payload: &[u8]) {
    let prefix = String::from_utf8_lossy(&payload[..payload.len().min(64 * 1024)]);
    let find = |key: &str, chars: usize| {
        let marker = format!("\"{key}\"");
        let start = prefix.find(&marker)? + marker.len();
        let rest = prefix[start..].trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        let end = rest.find('"')?;
        let value = &rest[..end];
        (!value.contains('\\')).then(|| preview(value, chars))?
    };
    record.event = find("hook_event_name", NAME_CHARS).unwrap_or_default();
    record.cli_session_id = find("session_id", ID_CHARS);
    record.agent_id = find("agent_id", ID_CHARS);
    record.tool = find("tool_name", ID_CHARS);
}

/// The text cut to `chars` characters on one line, or `None` when it is
/// empty.
pub fn preview(text: &str, chars: usize) -> Option<String> {
    let mut line = String::new();
    let mut count = 0;
    let mut last_was_space = true;
    for character in text.chars() {
        if count >= chars {
            break;
        }
        let character = if character.is_whitespace() || character.is_control() {
            if last_was_space {
                continue;
            }
            ' '
        } else {
            character
        };
        last_was_space = character == ' ';
        line.push(character);
        count += 1;
    }
    let line = line.trim_end();
    (!line.is_empty()).then(|| line.to_string())
}

/// Appends one line to the journal of the pane with one write call. A
/// journal past the size cap moves aside first. Claude Code runs the hooks
/// of parallel tool calls at the same time, so a lock keeps two helpers from
/// both moving the journal aside.
pub fn append_journal(status_dir: &Path, session_id: &str, line: &str) -> io::Result<()> {
    create_status_dir(status_dir)?;
    let path = journal_path(status_dir, session_id);
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    // The lock ends when the file closes at the end of this function.
    let _lock = lock_journal(status_dir, session_id);
    if fs::metadata(&path)
        .is_ok_and(|metadata| metadata.len() + bytes.len() as u64 > MAX_JOURNAL_BYTES)
    {
        fs::rename(&path, rotated_journal_path(status_dir, session_id))?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(&path)?.write_all(&bytes)
}

/// Takes the journal lock, or gives `None` after the time limit or on an
/// error.
fn lock_journal(status_dir: &Path, session_id: &str) -> Option<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(journal_lock_path(status_dir, session_id))
        .ok()?;
    let deadline = Instant::now() + JOURNAL_LOCK_TIME_LIMIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => return None,
        }
    }
}

fn create_status_dir(status_dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(status_dir)
}

/// Sends the pane id to the wake socket of the app. The app then reads the
/// journal. Errors do not matter: the app also polls the journals.
pub fn wake(status_dir: &Path, session_id: &str) {
    #[cfg(unix)]
    {
        let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() else {
            return;
        };
        // A full receive queue must not stall the agent.
        let _ = socket.set_nonblocking(true);
        let _ = socket.send_to(session_id.as_bytes(), status_dir.join(WAKE_SOCKET_NAME));
    }
    #[cfg(not(unix))]
    let _ = (status_dir, session_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const PANE: &str = "4b3f0c1e-7d2a-4f7e-9b8c-1a2b3c4d5e6f";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-agent-hook-{label}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parse(payload: &str) -> Option<HookRecord> {
        parse_payload(PANE, "claude", 7, payload.as_bytes(), false)
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn a_permission_request_keeps_the_tool_and_a_command_preview() {
        let record = parse(
            r#"{"session_id":"cli-1","transcript_path":"/x.jsonl","cwd":"/repo",
                "hook_event_name":"PermissionRequest","tool_name":"Bash",
                "tool_input":{"command":"npm test\n  --watch=false","description":"Run tests"},
                "permission_suggestions":[]}"#,
        )
        .expect("record");
        assert_eq!(
            record,
            HookRecord {
                v: 1,
                at: 7,
                pane: PANE.to_string(),
                agent: "claude".to_string(),
                event: "PermissionRequest".to_string(),
                cli_session_id: Some("cli-1".to_string()),
                tool: Some("Bash".to_string()),
                command: Some("npm test --watch=false".to_string()),
                ..HookRecord::default()
            }
        );
    }

    #[test]
    fn a_question_keeps_its_text_and_option_labels() {
        let record = parse(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
                "tool_use_id":"toolu_1","tool_input":{"questions":[
                  {"question":"Which store?","header":"Store","multiSelect":false,
                   "options":[{"label":"Redis","description":"fast"},{"label":"Postgres"}]},
                  {"question":"Second?","options":["a"]}]}}"#,
        )
        .expect("record");
        assert_eq!(record.question.as_deref(), Some("Which store?"));
        assert_eq!(
            record.options,
            Some(vec!["Redis".to_string(), "Postgres".to_string()])
        );
        assert_eq!(record.tool_use_id.as_deref(), Some("toolu_1"));
        let plain = parse(
            r#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion",
                "tool_input":{"questions":[{"question":"Pick","options":["a","b"]}]}}"#,
        )
        .expect("record");
        assert_eq!(plain.options, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn file_tools_keep_the_path_and_other_tools_keep_a_summary() {
        let edit = parse(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"Edit",
                "tool_input":{"file_path":"/repo/src/main.rs","old_string":"a","new_string":"b"}}"#,
        )
        .expect("record");
        assert_eq!(edit.file_path.as_deref(), Some("/repo/src/main.rs"));
        assert_eq!(edit.message, None);
        let fetch = parse(
            r#"{"hook_event_name":"PermissionRequest","tool_name":"WebFetch",
                "tool_input":{"url":"https://example.com","prompt":"secret prompt"}}"#,
        )
        .expect("record");
        assert_eq!(fetch.message.as_deref(), Some("https://example.com"));
        let line = serde_json::to_string(&fetch).unwrap();
        assert!(!line.contains("secret prompt"));
    }

    #[test]
    fn previews_are_cut_to_one_short_line() {
        let long = "x".repeat(PREVIEW_CHARS * 3);
        let record = parse(&format!(
            r#"{{"hook_event_name":"Notification","notification_type":"permission_prompt",
                "message":"{long}"}}"#
        ))
        .expect("record");
        assert_eq!(
            record.message.as_deref().map(|text| text.chars().count()),
            Some(PREVIEW_CHARS)
        );
        assert_eq!(preview("  a\t\tb \n c  ", 20).as_deref(), Some("a b c"));
        assert_eq!(preview(" \n ", 20), None);
        assert_eq!(preview("åäö€😀", 3).as_deref(), Some("åäö"));
    }

    #[test]
    fn outcome_fields_survive() {
        let stop = parse(
            r#"{"hook_event_name":"Stop","stop_hook_active":false,
                "last_assistant_message":"All tests pass."}"#,
        )
        .expect("record");
        assert_eq!(stop.stop_hook_active, Some(false));
        let line = serde_json::to_string(&stop).unwrap();
        assert!(!line.contains("All tests pass"), "{line}");

        let failure = parse(
            r#"{"hook_event_name":"StopFailure","error":"rate_limit","error_details":"429"}"#,
        )
        .expect("record");
        assert_eq!(failure.error.as_deref(), Some("rate_limit"));
        let nested = parse(r#"{"hook_event_name":"StopFailure","error":{"type":"overloaded"}}"#)
            .expect("record");
        assert_eq!(nested.error.as_deref(), Some("overloaded"));

        let start = parse(r#"{"hook_event_name":"SessionStart","source":"clear"}"#).unwrap();
        assert_eq!(start.source.as_deref(), Some("clear"));
        let subagent =
            parse(r#"{"hook_event_name":"SubagentStart","agent_id":"a9de","agent_type":"x"}"#)
                .unwrap();
        assert_eq!(subagent.agent_id.as_deref(), Some("a9de"));
    }

    #[test]
    fn invalid_payloads_give_no_record() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse("[1,2]"), None);
        assert_eq!(parse(r#"{"tool_name":"Bash"}"#), None);
        assert_eq!(parse(r#"{"hook_event_name":"Bad Name"}"#), None);
        assert_eq!(parse(r#"{"hook_event_name":"../etc"}"#), None);
    }

    #[test]
    fn a_cut_payload_keeps_its_event_and_tool() {
        let payload = format!(
            r#"{{"session_id":"cli-1","hook_event_name":"PostToolUse","tool_name":"Read","tool_response":"{}"#,
            "y".repeat(1000)
        );
        let record = parse_payload(PANE, "claude", 1, payload.as_bytes(), true).expect("record");
        assert_eq!(record.event, "PostToolUse");
        assert_eq!(record.tool.as_deref(), Some("Read"));
        assert_eq!(record.cli_session_id.as_deref(), Some("cli-1"));
        assert_eq!(
            parse_payload(PANE, "claude", 1, payload.as_bytes(), false),
            None
        );
    }

    #[test]
    fn a_large_input_is_cut_at_the_limit_and_drained() {
        let input = vec![b'a'; 10 * 1024 * 1024];
        let (kept, truncated) = read_limited(
            io::Cursor::new(input),
            MAX_STDIN_BYTES,
            Duration::from_secs(10),
        );
        assert_eq!(kept.len(), MAX_STDIN_BYTES);
        assert!(truncated);
        let (kept, truncated) =
            read_limited(io::Cursor::new(b"{}".to_vec()), 16, Duration::from_secs(10));
        assert_eq!(kept, b"{}");
        assert!(!truncated);
    }

    #[test]
    fn a_stalled_input_stops_at_the_time_limit() {
        struct Stall;
        impl Read for Stall {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                thread::sleep(Duration::from_secs(5));
                Ok(0)
            }
        }
        let started = Instant::now();
        let (kept, truncated) = read_limited(Stall, 16, Duration::from_millis(100));
        assert!(kept.is_empty());
        assert!(truncated);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_target_needs_both_variables_and_no_job_worker() {
        let complete = [(SESSION_ID_ENV, PANE), (STATUS_DIR_ENV, "/tmp/halv-status")];
        assert_eq!(
            HookTarget::from_lookup(lookup(&complete)),
            Some(HookTarget {
                session_id: PANE.to_string(),
                status_dir: PathBuf::from("/tmp/halv-status"),
            })
        );
        assert_eq!(HookTarget::from_lookup(lookup(&complete[..1])), None);
        assert_eq!(HookTarget::from_lookup(lookup(&complete[1..])), None);
        let mut worker = complete.to_vec();
        worker.push((JOB_DIR_ENV, "/tmp/job"));
        assert_eq!(HookTarget::from_lookup(lookup(&worker)), None);
        for bad in ["../x", "a/b", "", "a b"] {
            let pairs = [(SESSION_ID_ENV, bad), (STATUS_DIR_ENV, "/tmp/s")];
            assert_eq!(HookTarget::from_lookup(lookup(&pairs)), None, "{bad}");
        }
        let relative = [(SESSION_ID_ENV, PANE), (STATUS_DIR_ENV, "status")];
        assert_eq!(HookTarget::from_lookup(lookup(&relative)), None);
    }

    #[test]
    fn the_agent_argument_is_a_short_lowercase_name() {
        assert_eq!(agent_name(Some(OsStr::new("claude"))), "claude");
        assert_eq!(agent_name(Some(OsStr::new("codex"))), "codex");
        assert_eq!(agent_name(Some(OsStr::new("../x"))), "claude");
        assert_eq!(agent_name(None), "claude");
    }

    #[test]
    fn the_journal_appends_lines_and_rotates_at_the_cap() {
        let scratch = Scratch::new("journal");
        let dir = scratch.0.join("agent-status");
        append_journal(&dir, PANE, r#"{"a":1}"#).unwrap();
        append_journal(&dir, PANE, r#"{"a":2}"#).unwrap();
        let path = journal_path(&dir, PANE);
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}\n{\"a\":2}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir), 0o700);
        }

        let big = "z".repeat(600 * 1024);
        append_journal(&dir, PANE, &big).unwrap();
        append_journal(&dir, PANE, &big).unwrap();
        let rotated = rotated_journal_path(&dir, PANE);
        assert!(fs::metadata(&rotated).unwrap().len() < MAX_JOURNAL_BYTES);
        assert_eq!(
            fs::read_to_string(&path).unwrap().len(),
            big.len() + 1,
            "the new journal holds only the last line"
        );
    }

    #[test]
    fn parallel_helpers_rotate_the_journal_once() {
        let scratch = Scratch::new("parallel");
        let dir = scratch.0.join("agent-status");
        let near_cap = "p".repeat(MAX_JOURNAL_BYTES as usize - 2_000);
        append_journal(&dir, PANE, &near_cap).unwrap();
        let threads = (0..8)
            .map(|thread| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for line in 0..50 {
                        let text = format!(r#"{{"thread":{thread},"line":{line}}}"#);
                        append_journal(&dir, PANE, &text).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let count = |path: PathBuf| fs::read_to_string(path).unwrap().lines().count();
        let total = count(journal_path(&dir, PANE)) + count(rotated_journal_path(&dir, PANE));
        assert_eq!(total, 1 + 8 * 50, "no line is lost in a rotation");
        assert!(journal_lock_path(&dir, PANE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_wake_datagram_carries_the_pane_id() {
        use std::os::unix::net::UnixDatagram;
        let scratch = Scratch::new("wake");
        fs::create_dir_all(&scratch.0).unwrap();
        let receiver = UnixDatagram::bind(scratch.0.join(WAKE_SOCKET_NAME)).unwrap();
        wake(&scratch.0, PANE);
        let mut buffer = [0; 256];
        let count = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..count], PANE.as_bytes());
        // No listener is not an error for the helper.
        wake(&scratch.0.join("missing"), PANE);
    }
}
