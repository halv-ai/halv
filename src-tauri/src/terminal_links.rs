//! Resolves the path text of terminal links to paths on disk.
//!
//! A terminal prints untrusted text. This module only reads file metadata. It
//! never runs a file. A candidate gets the kind "file" or "dir" only when the
//! resolved path exists.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::terminal::{TerminalManager, TerminalSession};

/// The command resolves this number of candidates at most. Each extra
/// candidate gets the kind "none".
const MAX_CANDIDATES: usize = 64;
/// A candidate with more characters than this limit gets the kind "none".
const MAX_CANDIDATE_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalLinkKind {
    File,
    Dir,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLinkTarget {
    pub text: String,
    pub path: Option<String>,
    pub kind: TerminalLinkKind,
}

impl TerminalLinkTarget {
    fn none(text: &str) -> Self {
        Self {
            text: text.to_string(),
            path: None,
            kind: TerminalLinkKind::None,
        }
    }
}

/// Returns one target for each candidate, in the same order. A relative
/// candidate resolves against the live cwd of the shell process. When the
/// live cwd is not available, it resolves against the cwd of the session.
#[tauri::command]
pub async fn terminal_link_targets(
    manager: tauri::State<'_, TerminalManager>,
    session_id: String,
    candidates: Vec<String>,
) -> Result<Vec<TerminalLinkTarget>, String> {
    let session = manager.session(&session_id).ok();
    tauri::async_runtime::spawn_blocking(move || {
        let home = home_directory();
        resolve_candidates(
            &candidates,
            || session.as_ref().and_then(session_base),
            home.as_deref(),
        )
    })
    .await
    .map_err(|error| format!("failed to resolve the terminal links: {error}"))
}

/// Resolves the candidates. The function calls `base` one time at most, and
/// only when a candidate is relative.
fn resolve_candidates(
    candidates: &[String],
    base: impl FnOnce() -> Option<PathBuf>,
    home: Option<&Path>,
) -> Vec<TerminalLinkTarget> {
    let mut base = Some(base);
    let mut base_path: Option<PathBuf> = None;
    let mut relative_base = || {
        if let Some(read) = base.take() {
            base_path = read();
        }
        base_path.clone()
    };
    candidates
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if index >= MAX_CANDIDATES {
                return TerminalLinkTarget::none(text);
            }
            resolve_candidate(text, &mut relative_base, home)
                .and_then(|path| target_for(text, path))
                .unwrap_or_else(|| TerminalLinkTarget::none(text))
        })
        .collect()
}

fn resolve_candidate(
    text: &str,
    base: &mut impl FnMut() -> Option<PathBuf>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if text.is_empty()
        || text.chars().count() > MAX_CANDIDATE_CHARS
        || text.chars().any(char::is_control)
    {
        return None;
    }
    let path = if text == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = home_relative(text) {
        home?.join(rest)
    } else if text.starts_with('~') {
        // "~user" paths need a user lookup. Halv does not support them.
        return None;
    } else {
        let path = Path::new(text);
        if is_remote_path(path) {
            return None;
        }
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base()?.join(path)
        }
    };
    let path = normalize_lexically(&path)?;
    if is_remote_path(&path) {
        return None;
    }
    Some(path)
}

fn home_relative(text: &str) -> Option<&str> {
    text.strip_prefix("~/").or_else(|| {
        if cfg!(windows) {
            text.strip_prefix("~\\")
        } else {
            None
        }
    })
}

fn target_for(text: &str, path: PathBuf) -> Option<TerminalLinkTarget> {
    let metadata = std::fs::metadata(&path).ok()?;
    let kind = if metadata.is_dir() {
        TerminalLinkKind::Dir
    } else if metadata.is_file() {
        TerminalLinkKind::File
    } else {
        return None;
    };
    // A path that is not valid UTF-8 has no lossless string form.
    let path = path.into_os_string().into_string().ok()?;
    Some(TerminalLinkTarget {
        text: text.to_string(),
        path: Some(path),
        kind,
    })
}

/// Removes "." segments and applies ".." segments without a file system
/// read. A ".." segment at the root stays at the root. The result is `None`
/// when the path is not absolute.
pub(crate) fn normalize_lexically(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth > 0 {
                    normalized.pop();
                    depth -= 1;
                }
            }
            Component::Normal(segment) => {
                normalized.push(segment);
                depth += 1;
            }
        }
    }
    normalized.is_absolute().then_some(normalized)
}

/// True for a path that can make the system contact another host. A metadata
/// read of such a path can send credentials or start a network mount, so
/// Halv does not resolve it.
pub(crate) fn is_remote_path(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Prefix;
        if let Some(Component::Prefix(prefix)) = path.components().next() {
            return !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_));
        }
        false
    }
    #[cfg(not(windows))]
    {
        // The automounter mounts hosts below /net (macOS and Linux) and
        // /Network (macOS).
        let mut components = path.components();
        matches!(components.next(), Some(Component::RootDir))
            && matches!(
                components.next(),
                Some(Component::Normal(name)) if name == "net" || name == "Network"
            )
    }
}

/// The directory for relative candidates: the live cwd of the shell process,
/// else the cwd that started the session.
fn session_base(session: &TerminalSession) -> Option<PathBuf> {
    let live = if session.status == "running" {
        live_cwd(session.pid)
    } else {
        None
    };
    live.or_else(|| {
        let cwd = PathBuf::from(&session.cwd);
        normalize_lexically(&cwd)
    })
}

/// Reads the current directory of one process. Windows often gives no
/// value.
fn live_cwd(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        false,
        ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always),
    );
    let cwd = system.process(pid)?.cwd()?;
    normalize_lexically(cwd)
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let value: Option<OsString> = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value: Option<OsString> = std::env::var_os("HOME");
    value
        .map(PathBuf::from)
        .and_then(|home| normalize_lexically(&home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let root = std::env::temp_dir().join(format!(
                "halv-terminal-links-{name}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(root.join("src/nested")).expect("create temp tree");
            fs::write(root.join("src/main.rs"), "fn main() {}").expect("write file");
            fs::write(root.join("report.html"), "<p>ok</p>").expect("write file");
            Self(root)
        }

        fn path(&self, relative: &str) -> String {
            self.0.join(relative).to_string_lossy().into_owned()
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn resolve(
        candidates: &[&str],
        base: Option<&Path>,
        home: Option<&Path>,
    ) -> Vec<TerminalLinkTarget> {
        let candidates: Vec<String> = candidates.iter().map(|text| text.to_string()).collect();
        resolve_candidates(&candidates, || base.map(Path::to_path_buf), home)
    }

    #[test]
    fn relative_candidates_resolve_against_the_base_directory() {
        let tree = TempTree::new("relative");
        let targets = resolve(
            &[
                "src/main.rs",
                "./src/nested",
                "src/nested/../main.rs",
                "missing.txt",
            ],
            Some(&tree.0),
            None,
        );
        assert_eq!(targets.len(), 4);
        assert_eq!(targets[0].kind, TerminalLinkKind::File);
        assert_eq!(
            targets[0].path.as_deref(),
            Some(tree.path("src/main.rs").as_str())
        );
        assert_eq!(targets[0].text, "src/main.rs");
        assert_eq!(targets[1].kind, TerminalLinkKind::Dir);
        assert_eq!(
            targets[1].path.as_deref(),
            Some(tree.path("src/nested").as_str())
        );
        assert_eq!(targets[2].kind, TerminalLinkKind::File);
        assert_eq!(
            targets[2].path.as_deref(),
            Some(tree.path("src/main.rs").as_str())
        );
        assert_eq!(targets[3], TerminalLinkTarget::none("missing.txt"));
    }

    #[test]
    fn absolute_and_home_candidates_resolve_without_a_base() {
        let tree = TempTree::new("absolute");
        let absolute = tree.path("report.html");
        let targets = resolve(
            &[&absolute, "~/src/main.rs", "~", "~nobody/x", "src/main.rs"],
            None,
            Some(&tree.0),
        );
        assert_eq!(targets[0].kind, TerminalLinkKind::File);
        assert_eq!(targets[0].path.as_deref(), Some(absolute.as_str()));
        assert_eq!(targets[1].kind, TerminalLinkKind::File);
        assert_eq!(
            targets[1].path.as_deref(),
            Some(tree.path("src/main.rs").as_str())
        );
        assert_eq!(targets[2].kind, TerminalLinkKind::Dir);
        assert_eq!(targets[3].kind, TerminalLinkKind::None);
        // No base directory, so a relative candidate does not resolve.
        assert_eq!(targets[4].kind, TerminalLinkKind::None);
    }

    #[test]
    fn the_base_directory_is_read_only_for_relative_candidates() {
        let tree = TempTree::new("lazy");
        let absolute = tree.path("report.html");
        let mut calls = 0;
        let targets = resolve_candidates(
            &[absolute.clone(), absolute],
            || {
                calls += 1;
                None
            },
            None,
        );
        assert_eq!(calls, 0);
        assert!(targets
            .iter()
            .all(|target| target.kind == TerminalLinkKind::File));

        let mut calls = 0;
        resolve_candidates(
            &["a".to_string(), "b".to_string()],
            || {
                calls += 1;
                None
            },
            None,
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn caps_and_bad_text_give_none() {
        let tree = TempTree::new("caps");
        let mut candidates = vec!["src/main.rs".to_string(); MAX_CANDIDATES + 2];
        candidates.push("x".repeat(MAX_CANDIDATE_CHARS + 1));
        let targets = resolve_candidates(&candidates, || Some(tree.0.clone()), None);
        assert_eq!(targets.len(), MAX_CANDIDATES + 3);
        assert!(targets[..MAX_CANDIDATES]
            .iter()
            .all(|target| target.kind == TerminalLinkKind::File));
        assert!(targets[MAX_CANDIDATES..]
            .iter()
            .all(|target| target.kind == TerminalLinkKind::None && target.path.is_none()));

        let long = format!("src/{}", "a".repeat(MAX_CANDIDATE_CHARS));
        let targets = resolve(
            &["", "src/main.rs\n", "src/\u{1b}main.rs", &long],
            Some(&tree.0),
            None,
        );
        assert!(targets
            .iter()
            .all(|target| target.kind == TerminalLinkKind::None));
    }

    #[test]
    fn parent_segments_stop_at_the_root() {
        #[cfg(not(windows))]
        {
            assert_eq!(
                normalize_lexically(Path::new("/a/./b/../../../c")),
                Some(PathBuf::from("/c"))
            );
            assert_eq!(normalize_lexically(Path::new("a/b")), None);
        }
        #[cfg(windows)]
        {
            assert_eq!(
                normalize_lexically(Path::new(r"C:\a\.\b\..\..\..\c")),
                Some(PathBuf::from(r"C:\c"))
            );
            assert_eq!(normalize_lexically(Path::new(r"a\b")), None);
        }
    }

    #[test]
    fn remote_paths_do_not_resolve() {
        #[cfg(not(windows))]
        {
            assert!(is_remote_path(Path::new("/net/example.com/share")));
            assert!(is_remote_path(Path::new("/Network/Servers/x")));
            assert!(!is_remote_path(Path::new("/Users/me/net/x")));
            let targets = resolve(
                &["/net/example.com/x", "../../net/host/x"],
                Some(Path::new("/a/b")),
                None,
            );
            assert!(targets
                .iter()
                .all(|target| target.kind == TerminalLinkKind::None));
        }
        #[cfg(windows)]
        {
            assert!(is_remote_path(Path::new(r"\\example.com\share\x")));
            assert!(is_remote_path(Path::new(r"\\?\UNC\example.com\share\x")));
            assert!(!is_remote_path(Path::new(r"C:\Users\me")));
        }
    }

    #[test]
    fn targets_serialize_with_the_ipc_shape() {
        let value = serde_json::to_value(TerminalLinkTarget::none("a.txt")).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({ "text": "a.txt", "path": null, "kind": "none" })
        );
    }

    #[test]
    fn live_cwd_reads_the_current_process() {
        let expected = normalize_lexically(&std::env::current_dir().expect("current dir"));
        let live = live_cwd(std::process::id());
        // Windows can give no value.
        if cfg!(not(windows)) {
            assert_eq!(live, expected);
        }
        assert_eq!(live_cwd(0), None);
    }
}
