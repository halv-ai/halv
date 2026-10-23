//! A short log of the banners that Halv sends.
//!
//! A user reported more banners than agent events. The log names the pane
//! signal behind each banner, so a later check can find the source. It keeps
//! no title and no body. A bounded file in the logs folder holds the lines.

#![cfg_attr(not(desktop), allow(dead_code))]

use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::Mutex,
};

pub(crate) const FILE_NAME: &str = "notifications.log";
const MAX_BYTES: usize = 128 * 1024;
const MAX_FIELD_CHARS: usize = 96;

static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Makes a value safe for one `key=value` field of a log line.
fn field(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .take(MAX_FIELD_CHARS)
        .collect();
    if cleaned.is_empty() {
        "-".to_string()
    } else {
        cleaned
    }
}

/// Builds the line for one banner.
pub(crate) fn banner_line(
    timestamp: &str,
    session_id: &str,
    kind: Option<&str>,
    trace: Option<&str>,
    os_state: &str,
) -> String {
    format!(
        "{timestamp} banner session={} kind={} trace={} os={}",
        field(if session_id.is_empty() {
            "test"
        } else {
            session_id
        }),
        field(kind.unwrap_or("-")),
        field(trace.unwrap_or("-")),
        field(os_state),
    )
}

/// Keeps the newest lines that fit in half of the limit.
fn newest_half(content: &str) -> &str {
    let keep = MAX_BYTES / 2;
    if content.len() <= keep {
        return content;
    }
    let mut start = content.len() - keep;
    while !content.is_char_boundary(start) {
        start += 1;
    }
    match content[start..].find('\n') {
        Some(newline) => &content[start + newline + 1..],
        None => "",
    }
}

/// Appends one line. A file above the limit loses its oldest lines first.
pub(crate) fn append(logs_dir: &Path, line: &str) -> io::Result<()> {
    let _guard = WRITE_LOCK
        .lock()
        .map_err(|_| io::Error::other("notification log lock poisoned"))?;
    fs::create_dir_all(logs_dir)?;
    let path = logs_dir.join(FILE_NAME);
    let existing = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        // A file that is not text starts again.
        Err(error) if error.kind() == io::ErrorKind::InvalidData => String::new(),
        Err(error) => return Err(error),
    };
    if existing.len() + line.len() + 1 > MAX_BYTES {
        let mut kept = newest_half(&existing).to_string();
        kept.push_str(line);
        kept.push('\n');
        return fs::write(&path, kept);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "halv-notification-log-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_line_names_the_signal_and_never_the_text() {
        let line = banner_line(
            "2026-10-22T16:00:00Z",
            "pane-1",
            Some("completion"),
            Some("presence:done:wait0:turn3"),
            "denied",
        );
        assert_eq!(
            line,
            "2026-10-22T16:00:00Z banner session=pane-1 kind=completion \
             trace=presence:done:wait0:turn3 os=denied"
        );
    }

    #[test]
    fn a_field_cannot_break_the_line() {
        let line = banner_line("t", "", None, Some("a b\nc\td"), "granted");
        assert!(!line.contains('\n'));
        assert!(line.contains("session=test"));
        assert!(line.contains("kind=-"));
        assert!(line.contains("trace=a_b_c_d"));
        let long = banner_line("t", &"x".repeat(500), None, None, "granted");
        assert!(long.len() < 200);
    }

    #[test]
    fn the_file_keeps_its_newest_lines_within_the_limit() {
        let dir = scratch("bound");
        let line = banner_line(
            "t",
            "pane",
            Some("completion"),
            Some(&"y".repeat(90)),
            "granted",
        );
        let count = MAX_BYTES / line.len() * 2;
        for index in 0..count {
            append(&dir, &format!("{index} {line}")).unwrap();
        }
        let content = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(
            content.len() <= MAX_BYTES,
            "the file is {} bytes",
            content.len()
        );
        let last = content.lines().last().unwrap();
        assert!(last.starts_with(&format!("{} ", count - 1)));
        let first = content.lines().next().unwrap();
        assert!(!first.starts_with("0 "), "the oldest line stays");
        // Every kept line is whole.
        assert!(content.lines().all(|l| l.ends_with("os=granted")));
        let _ = fs::remove_dir_all(dir);
    }
}
