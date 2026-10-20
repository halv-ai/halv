//! The Claude hooks of the agent panes.
//!
//! The per-session Claude settings file of a pane runs the `agent-hook`
//! helper for each hook event. The helper appends one line per event to
//! `<hooks dir>/<session id>.jsonl` and sends the session id to
//! `<hooks dir>/wake.sock`. This module installs a stable copy of the
//! helper, builds the hook entries of the settings file, and reads the
//! journals.

pub(crate) use halv_supervisor::agent_hook::HookRecord;
use halv_supervisor::agent_hook::{
    is_safe_session_id, journal_lock_path, journal_path, rotated_journal_path, AGENT_HOOK_COMMAND,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// The helper copy that hook entries run. A stable path survives app
/// updates, macOS App Translocation, and AppImage mounts.
const HELPER_DIR: &str = "bin";
const HELPER_NAME: &str = "halv-agent-hook";
/// A journal read takes this much at a time until the end of the file. The
/// helper keeps each journal below 1 MiB.
const READ_CHUNK_BYTES: u64 = 256 * 1024;
/// Journals of panes that ended while the app was closed stay this long.
const JOURNAL_LIFETIME: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const HOOK_TIMEOUT_SECS: u64 = 5;

/// The hook events that Halv registers, with the first Claude Code version
/// that knows each one. Claude Code 1.0.23 to 2.1.100 drop the whole
/// settings file when it names an unknown event. Halv never registers
/// `Elicitation`: a hook answer can answer the MCP dialog.
const CLAUDE_HOOK_EVENTS: [(&str, Version); 12] = [
    ("PreToolUse", (1, 0, 23)),
    ("PostToolUse", (1, 0, 23)),
    ("Notification", (1, 0, 23)),
    ("Stop", (1, 0, 31)),
    ("SubagentStop", (1, 0, 41)),
    ("UserPromptSubmit", (1, 0, 53)),
    ("SessionStart", (1, 0, 62)),
    ("SessionEnd", (1, 0, 85)),
    ("SubagentStart", (2, 0, 43)),
    ("PermissionRequest", (2, 0, 45)),
    ("PostToolUseFailure", (2, 0, 56)),
    ("StopFailure", (2, 1, 78)),
];
/// Events that take a tool name matcher.
const TOOL_EVENTS: [&str; 4] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
];
/// The version that Halv assumes when the probe fails. Every event up to it
/// is safe for the Claude Code releases that Halv supports.
const UNKNOWN_VERSION: Version = (1, 0, 64);

type Version = (u64, u64, u64);

/// The status hook entries of one Claude launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudeStatusHooks {
    command: String,
    events: Vec<&'static str>,
}

impl ClaudeStatusHooks {
    /// `version` is the text that `claude --version` prints, if known.
    pub(crate) fn new(helper: &Path, version: Option<&str>) -> Result<Self, String> {
        let version = version.and_then(parse_version).unwrap_or(UNKNOWN_VERSION);
        Ok(Self {
            command: format!("{} {AGENT_HOOK_COMMAND} claude", shell_quote(helper)?),
            events: CLAUDE_HOOK_EVENTS
                .iter()
                .filter(|(_, first)| version >= *first)
                .map(|(event, _)| *event)
                .collect(),
        })
    }

    /// One matcher group for each event, ready for the `hooks` object of a
    /// Claude settings file.
    pub(crate) fn groups(&self) -> impl Iterator<Item = (&'static str, Value)> + '_ {
        self.events.iter().map(|event| {
            let hook = json!({
                "type": "command",
                "command": self.command,
                "timeout": HOOK_TIMEOUT_SECS,
            });
            let group = if TOOL_EVENTS.contains(event) {
                json!({ "matcher": "*", "hooks": [hook] })
            } else {
                json!({ "hooks": [hook] })
            };
            (*event, group)
        })
    }
}

/// The status hooks for a Claude launch of `program`. `None` when the
/// helper copy is missing; the presence file still works then.
/// `probe_version` gives the `--version` text of `program`. The launch
/// calls it only when the install path does not show the version.
pub(crate) fn claude_status_hooks_for(
    program: &Path,
    probe_version: impl FnOnce() -> Option<String>,
) -> Option<ClaudeStatusHooks> {
    let helper = super::helper_path()?;
    let version = claude_version(program, probe_version);
    match ClaudeStatusHooks::new(helper, version.as_deref()) {
        Ok(hooks) => Some(hooks),
        Err(error) => {
            eprintln!("agent status: no Claude hooks for this launch: {error}");
            None
        }
    }
}

/// Quotes a path for `/bin/sh`. Claude Code runs the hook command in a
/// shell.
fn shell_quote(path: &Path) -> Result<String, String> {
    let text = path
        .to_str()
        .ok_or_else(|| "the hook helper path is not valid UTF-8".to_string())?;
    if text.contains(['\0', '\r', '\n']) {
        return Err("the hook helper path contains a line break".to_string());
    }
    if !text.is_empty()
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character))
    {
        return Ok(text.to_string());
    }
    Ok(format!("'{}'", text.replace('\'', "'\\''")))
}

/// The first `x.y.z` in `text`.
fn parse_version(text: &str) -> Option<Version> {
    text.split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .find_map(|token| {
            let mut parts = token.split('.');
            let major = parts.next()?.parse().ok()?;
            let minor = parts.next()?.parse().ok()?;
            let patch = parts.next()?.parse().ok()?;
            Some((major, minor, patch))
        })
}

/// The version folders of Claude Code installs: the native installer
/// (`claude/versions/2.1.299`), the Homebrew cask
/// (`Caskroom/claude-code/2.1.299`), and the Halv install
/// (`bin/claude/2.1.299`). A version in another path can belong to Node,
/// to a version manager, or to Homebrew itself.
fn version_from_path(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    parts.iter().enumerate().rev().find_map(|(index, text)| {
        let core = text.split(['-', '+']).next()?;
        let numbers = core.split('.').collect::<Vec<_>>();
        let is_version = numbers.len() == 3
            && numbers
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        let parent = |back: usize| index.checked_sub(back).map(|at| parts[at]);
        let claude_folder = match parent(1)? {
            "versions" => parent(2) == Some("claude"),
            "claude-code" => parent(2) == Some("Caskroom"),
            "claude" => parent(2) == Some("bin"),
            _ => false,
        };
        (is_version && claude_folder).then(|| core.to_string())
    })
}

/// The Claude Code version of `program`. It reads the version from the
/// install path when it can. Else it takes the shared `--version` probe of
/// the agent list, which runs the CLI at most once per binary and minute.
fn claude_version(
    program: &Path,
    probe_version: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let resolved = fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    version_from_path(&resolved).or_else(probe_version)
}

/// Copies the hook helper to its stable path when the bytes differ.
#[cfg(unix)]
pub(crate) fn install_helper(hooks_dir: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let source = crate::supervisor_link::locate_supervisor_binary()?;
    let target = hooks_dir.join(HELPER_DIR).join(HELPER_NAME);
    let bytes = fs::read(&source)
        .map_err(|error| format!("could not read {}: {error}", source.display()))?;
    if fs::read(&target).is_ok_and(|existing| existing == bytes) {
        return Ok(target);
    }
    let parent = target.parent().unwrap_or(hooks_dir);
    create_private_dir(parent)?;
    // A new file has no quarantine attribute. A rename keeps the old inode
    // for hooks that run at this moment.
    let temporary = parent.join(format!("{HELPER_NAME}.{}.tmp", std::process::id()));
    fs::write(&temporary, &bytes)
        .and_then(|()| fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)))
        .and_then(|()| fs::rename(&temporary, &target))
        .map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!("could not install {}: {error}", target.display())
        })?;
    Ok(target)
}

/// Creates a folder that only the user can open.
pub(crate) fn create_private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder
            .create(dir)
            .and_then(|()| fs::set_permissions(dir, fs::Permissions::from_mode(0o700)))
            .map_err(|error| format!("could not create {}: {error}", dir.display()))
    }
    #[cfg(not(unix))]
    builder
        .create(dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))
}

/// Removes the journals of panes that ended long ago.
pub(crate) fn remove_old_journals(hooks_dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(hooks_dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if ![".jsonl", ".jsonl.1", ".jsonl.lock"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > JOURNAL_LIFETIME)
            });
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Removes the journals of one pane.
pub(crate) fn remove_journals(hooks_dir: &Path, session_id: &str) {
    if !is_safe_session_id(session_id) {
        return;
    }
    let _ = fs::remove_file(journal_path(hooks_dir, session_id));
    let _ = fs::remove_file(rotated_journal_path(hooks_dir, session_id));
    let _ = fs::remove_file(journal_lock_path(hooks_dir, session_id));
}

/// Reads the new lines of one pane journal.
#[derive(Debug)]
pub(crate) struct JournalTail {
    dir: PathBuf,
    session_id: String,
    offset: u64,
    /// The file that `offset` belongs to.
    identity: Option<(u64, u64)>,
    /// The size and file of the last read, to skip unchanged journals.
    seen: Option<(u64, Option<(u64, u64)>)>,
}

impl JournalTail {
    pub(crate) fn new(hooks_dir: &Path, session_id: &str) -> Option<Self> {
        is_safe_session_id(session_id).then(|| Self {
            dir: hooks_dir.to_path_buf(),
            session_id: session_id.to_string(),
            offset: 0,
            identity: None,
            seen: None,
        })
    }

    /// The complete lines after the last read. A partial last line waits
    /// for the next read.
    pub(crate) fn read_new(&mut self) -> Vec<HookRecord> {
        let path = journal_path(&self.dir, &self.session_id);
        let Ok(metadata) = fs::metadata(&path) else {
            return Vec::new();
        };
        let identity = file_identity(&metadata);
        let seen = (metadata.len(), identity);
        if self.seen == Some(seen) {
            return Vec::new();
        }
        let mut records = Vec::new();
        if self.identity.is_some() && self.identity != identity {
            // The helper moved the journal aside. Finish the old file first.
            let rotated = rotated_journal_path(&self.dir, &self.session_id);
            if fs::metadata(&rotated).is_ok_and(|old| file_identity(&old) == self.identity) {
                records.extend(read_lines(&rotated, &mut self.offset));
            }
            self.offset = 0;
        }
        if metadata.len() < self.offset {
            self.offset = 0;
        }
        self.identity = identity;
        records.extend(read_lines(&path, &mut self.offset));
        // A partial line changes the size later, so keep it unread.
        if self.offset == metadata.len() {
            self.seen = Some(seen);
        }
        records
    }
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// The complete lines from `offset` to the end of the file. A partial last
/// line stays unread.
fn read_lines(path: &Path, offset: &mut u64) -> Vec<HookRecord> {
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    loop {
        let mut bytes = Vec::new();
        let read = file
            .seek(SeekFrom::Start(*offset))
            .and_then(|_| (&mut file).take(READ_CHUNK_BYTES).read_to_end(&mut bytes));
        if read.is_err() {
            break;
        }
        let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            if bytes.len() as u64 == READ_CHUNK_BYTES {
                // One line is longer than a chunk. The helper never writes one.
                *offset += READ_CHUNK_BYTES;
                continue;
            }
            break;
        };
        *offset += end as u64 + 1;
        records.extend(
            bytes[..end]
                .split(|byte| *byte == b'\n')
                .filter_map(|line| serde_json::from_slice::<HookRecord>(line).ok()),
        );
    }
    records
}

/// Binds the wake socket. Each datagram holds the session id of a pane
/// with a new journal line.
#[cfg(unix)]
pub(crate) fn bind_wake_socket(hooks_dir: &Path) -> io::Result<std::os::unix::net::UnixDatagram> {
    use std::os::unix::fs::PermissionsExt;
    let path = hooks_dir.join(halv_supervisor::agent_hook::WAKE_SOCKET_NAME);
    // A socket file from an earlier app run blocks the bind.
    let _ = fs::remove_file(&path);
    let socket = std::os::unix::net::UnixDatagram::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use halv_supervisor::agent_hook::append_journal;
    use std::collections::HashMap;

    const PANE: &str = "11111111-2222-4333-8444-555555555555";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "halv-hooks-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn line(event: &str) -> String {
        format!(r#"{{"v":1,"at":1,"pane":"{PANE}","agent":"claude","event":"{event}"}}"#)
    }

    fn events(records: &[HookRecord]) -> Vec<&str> {
        records.iter().map(|record| record.event.as_str()).collect()
    }

    #[test]
    fn status_hooks_follow_the_claude_version() {
        let helper = Path::new("/opt/halv/bin/halv-agent-hook");
        let names = |version: Option<&str>| {
            ClaudeStatusHooks::new(helper, version)
                .unwrap()
                .groups()
                .map(|(event, _)| event)
                .collect::<Vec<_>>()
        };
        let all = names(Some("2.1.299 (Claude Code)"));
        assert_eq!(all.len(), 12);
        assert!(all.contains(&"StopFailure"));
        assert!(!all.contains(&"Elicitation"));
        assert!(!all.contains(&"PostCompact"));
        let older = names(Some("2.0.50"));
        assert!(older.contains(&"PermissionRequest"));
        assert!(!older.contains(&"PostToolUseFailure"));
        assert!(!older.contains(&"StopFailure"));
        assert_eq!(
            names(None),
            [
                "PreToolUse",
                "PostToolUse",
                "Notification",
                "Stop",
                "SubagentStop",
                "UserPromptSubmit",
                "SessionStart"
            ]
        );
        assert_eq!(
            names(Some("1.0.30")),
            ["PreToolUse", "PostToolUse", "Notification"]
        );
    }

    #[test]
    fn status_hook_groups_have_the_claude_shape() {
        let hooks = ClaudeStatusHooks::new(
            Path::new("/Users/me/Library/Application Support/ai.halv.app/hook's/halv-agent-hook"),
            Some("2.1.299"),
        )
        .unwrap();
        let groups = hooks.groups().collect::<HashMap<_, _>>();
        let command = "'/Users/me/Library/Application Support/ai.halv.app/hook'\\''s/halv-agent-hook' agent-hook claude";
        assert_eq!(
            groups["PermissionRequest"],
            json!({
                "matcher": "*",
                "hooks": [{ "type": "command", "command": command, "timeout": 5 }],
            })
        );
        assert_eq!(
            groups["Stop"],
            json!({ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] })
        );
        assert_eq!(
            ClaudeStatusHooks::new(Path::new("/opt/bin/halv-agent-hook"), None)
                .unwrap()
                .command,
            "/opt/bin/halv-agent-hook agent-hook claude"
        );
        assert!(ClaudeStatusHooks::new(Path::new("/bad\npath"), None).is_err());
    }

    #[test]
    fn versions_come_from_text_and_install_paths() {
        assert_eq!(parse_version("2.1.299 (Claude Code)"), Some((2, 1, 299)));
        assert_eq!(parse_version("claude 1.0.85-beta"), Some((1, 0, 85)));
        assert_eq!(parse_version("no version"), None);
        assert_eq!(
            version_from_path(Path::new("/Users/me/.local/share/claude/versions/2.1.299"))
                .as_deref(),
            Some("2.1.299")
        );
        assert_eq!(
            version_from_path(Path::new(
                "/opt/homebrew/Caskroom/claude-code/2.1.50/claude"
            ))
            .as_deref(),
            Some("2.1.50")
        );
        assert_eq!(
            version_from_path(Path::new(
                "/Users/me/Library/Application Support/ai.halv.app/bin/claude/2.1.200/claude"
            ))
            .as_deref(),
            Some("2.1.200")
        );
        // Versions of Node, of a version manager, or of Homebrew itself.
        for path in [
            "/Users/me/.nvm/versions/node/v20.11.0/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "/Users/me/.local/share/mise/installs/node/22.11.0/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "/Users/me/.asdf/installs/nodejs/20.11.0/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "/opt/homebrew/Cellar/mise/2025.10.3/bin/mise",
            "/Users/me/.local/share/fnm/node-versions/v21.4.0/installation/bin/claude",
        ] {
            assert_eq!(version_from_path(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn the_tail_reads_complete_lines_once() {
        let scratch = Scratch::new("tail");
        let mut tail = JournalTail::new(&scratch.0, PANE).unwrap();
        assert!(tail.read_new().is_empty());
        append_journal(&scratch.0, PANE, &line("SessionStart")).unwrap();
        append_journal(&scratch.0, PANE, &line("UserPromptSubmit")).unwrap();
        assert_eq!(
            events(&tail.read_new()),
            ["SessionStart", "UserPromptSubmit"]
        );
        assert!(tail.read_new().is_empty());

        // A partial line waits for its end.
        let path = journal_path(&scratch.0, PANE);
        let partial = line("Stop");
        let (head, rest) = partial.split_at(10);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| io::Write::write_all(&mut file, head.as_bytes()))
            .unwrap();
        assert!(tail.read_new().is_empty());
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                io::Write::write_all(&mut file, format!("{rest}\nnot json\n").as_bytes())
            })
            .unwrap();
        assert_eq!(events(&tail.read_new()), ["Stop"]);
    }

    #[test]
    fn the_tail_follows_a_rotation() {
        let scratch = Scratch::new("rotate");
        let mut tail = JournalTail::new(&scratch.0, PANE).unwrap();
        append_journal(&scratch.0, PANE, &line("SessionStart")).unwrap();
        assert_eq!(events(&tail.read_new()), ["SessionStart"]);
        append_journal(&scratch.0, PANE, &line("PreToolUse")).unwrap();
        fs::rename(
            journal_path(&scratch.0, PANE),
            rotated_journal_path(&scratch.0, PANE),
        )
        .unwrap();
        append_journal(&scratch.0, PANE, &line("Stop")).unwrap();
        assert_eq!(events(&tail.read_new()), ["PreToolUse", "Stop"]);
    }

    #[test]
    fn the_tail_drains_a_long_journal_and_a_long_rotated_one() {
        let scratch = Scratch::new("drain");
        let mut tail = JournalTail::new(&scratch.0, PANE).unwrap();
        append_journal(&scratch.0, PANE, &line("SessionStart")).unwrap();
        assert_eq!(tail.read_new().len(), 1);
        // More than three chunks are unread when the journal moves aside.
        let lines = (READ_CHUNK_BYTES as usize * 3) / line("PreToolUse").len() + 10;
        for _ in 0..lines {
            append_journal(&scratch.0, PANE, &line("PreToolUse")).unwrap();
        }
        fs::rename(
            journal_path(&scratch.0, PANE),
            rotated_journal_path(&scratch.0, PANE),
        )
        .unwrap();
        for _ in 0..lines {
            append_journal(&scratch.0, PANE, &line("PostToolUse")).unwrap();
        }
        let records = tail.read_new();
        assert_eq!(records.len(), lines * 2);
        assert_eq!(records.last().unwrap().event, "PostToolUse");
        assert!(tail.read_new().is_empty());
    }

    #[test]
    fn unsafe_session_ids_get_no_tail() {
        assert!(JournalTail::new(Path::new("/tmp"), "../x").is_none());
    }

    #[test]
    fn old_journals_go_and_a_pane_removes_its_own() {
        let scratch = Scratch::new("sweep");
        append_journal(&scratch.0, PANE, &line("Stop")).unwrap();
        fs::write(scratch.0.join("keep.txt"), "x").unwrap();
        remove_old_journals(&scratch.0, SystemTime::now());
        assert!(journal_path(&scratch.0, PANE).exists());
        remove_old_journals(&scratch.0, SystemTime::now() + JOURNAL_LIFETIME * 2);
        assert!(!journal_path(&scratch.0, PANE).exists());
        assert!(scratch.0.join("keep.txt").exists());

        append_journal(&scratch.0, PANE, &line("Stop")).unwrap();
        remove_journals(&scratch.0, PANE);
        assert!(!journal_path(&scratch.0, PANE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_wake_socket_receives_the_helper_datagram() {
        let scratch = Scratch::new("wake");
        fs::write(scratch.0.join("wake.sock"), "stale").unwrap();
        let socket = bind_wake_socket(&scratch.0).unwrap();
        halv_supervisor::agent_hook::wake(&scratch.0, PANE);
        let mut buffer = [0; 128];
        let count = socket.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..count], PANE.as_bytes());
    }
}
