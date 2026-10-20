//! Private config files of each agent session.
//!
//! The agent CLIs load these files outside the chat sandbox: the nested
//! `CODEX_HOME`, the Claude settings file, and the Kimi agent and MCP files.
//! A sandboxed command can write to the temp directory, so Halv keeps the
//! files under the app data directory. No sandbox write root contains that
//! directory.
//!
//! Short CLI runs, such as model discovery, also use a private working
//! directory here. A CLI can load project config from its working directory.

use crate::storage::{new_uuid_v4, Storage};
use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::{Duration, SystemTime},
};

const ROOT_DIRECTORY_NAME: &str = "agent-sessions";
pub(super) const CODEX_HOME_DIRECTORY_NAME: &str = "codex-home";
pub(super) const CLAUDE_SETTINGS_FILE_NAME: &str = "claude-settings.json";
pub(super) const KIMI_AGENT_FILE_NAME: &str = "kimi-agent.yaml";
pub(super) const KIMI_MCP_FILE_NAME: &str = "kimi-mcp.json";
// These statuses match `agent_session_is_live`.
const LIVE_SESSION_STATUSES: [&str; 3] = ["starting", "running", "waiting_input"];
// An agent in a Ghostty window can continue after the app marks its session
// as exited. The cleanup keeps each directory for this period.
const STALE_DIRECTORY_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

static ROOT: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn initialize_root(app_data_dir: &Path) {
    ROOT.get_or_init(|| root_for(app_data_dir));
}

fn root_for(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(ROOT_DIRECTORY_NAME)
}

pub(super) fn root() -> Result<PathBuf, String> {
    if let Some(root) = ROOT.get() {
        return Ok(root.clone());
    }
    // Unit tests run without the app setup. Other builds never use the temp
    // directory, because a sandboxed command can write there.
    #[cfg(test)]
    return Ok(std::env::temp_dir().join(format!("halv-session-files-test-{}", std::process::id())));
    #[cfg(not(test))]
    Err("the agent session directory is not ready".to_string())
}

/// Returns the private directory of one session. The function creates the
/// directory when it does not exist.
pub(super) fn directory(session_id: &str) -> Result<PathBuf, String> {
    let root = root()?;
    let directory = root.join(directory_name(session_id));
    create_private_directory(&root)?;
    create_private_directory(&directory)?;
    Ok(directory)
}

/// Removes the private directory of one session after its agent exits.
pub(super) fn remove(session_id: &str) {
    if let Ok(root) = root() {
        remove_directory(&root.join(directory_name(session_id)));
    }
}

/// The private working directory of one short CLI run outside the sandbox.
/// Drop removes the directory. When the app stops first, the stale cleanup
/// removes the directory later.
pub(super) struct WorkDirectory(PathBuf);

impl WorkDirectory {
    /// Creates a new directory. `purpose` starts the directory name.
    pub(super) fn create(purpose: &str) -> Result<Self, String> {
        directory(&work_directory_name(purpose)).map(Self)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDirectory {
    fn drop(&mut self) {
        remove_directory(&self.0);
    }
}

fn work_directory_name(purpose: &str) -> String {
    directory_name(&format!("{purpose}-{}", new_uuid_v4()))
}

/// Removes the old directories of sessions that storage does not list as
/// live. Call it during app setup, before a session can start. The directory
/// scan runs in the caller, so the background removal cannot delete the files
/// of a new session.
pub(crate) fn spawn_stale_cleanup(storage: &Storage) {
    let Ok(root) = root() else {
        return;
    };
    let mut live_directories = HashSet::new();
    for status in LIVE_SESSION_STATUSES {
        match storage.list_agent_sessions_with_status(status) {
            Ok(rows) => {
                live_directories.extend(rows.iter().map(|row| directory_name(&row.id)));
            }
            Err(error) => {
                eprintln!("session files: failed to list live sessions: {error}");
                return;
            }
        }
    }
    let cutoff = SystemTime::now()
        .checked_sub(STALE_DIRECTORY_AGE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let stale = stale_directories(&root, &live_directories, cutoff);
    if stale.is_empty() {
        return;
    }
    let spawn_result = thread::Builder::new()
        .name("session-files-cleanup".to_string())
        .spawn(move || {
            for directory in &stale {
                remove_directory(directory);
            }
        });
    if let Err(error) = spawn_result {
        eprintln!("session files: failed to start cleanup: {error}");
    }
}

fn stale_directories(
    root: &Path,
    live_directories: &HashSet<String>,
    cutoff: SystemTime,
) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .filter(|entry| !live_directories.contains(entry.file_name().to_string_lossy().as_ref()))
        .filter(|entry| {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified < cutoff)
        })
        .map(|entry| entry.path())
        .collect()
}

fn directory_name(session_id: &str) -> String {
    let name = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if name.is_empty() {
        "session".to_string()
    } else {
        name
    }
}

fn create_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "failed to create the session directory `{}`: {error}",
            path.display()
        )
    })?;
    restrict_directory_permissions(path)
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!(
            "failed to protect the session directory `{}`: {error}",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn remove_directory(path: &Path) {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "failed to remove the session directory `{}`: {error}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::WorkspaceRoots;

    #[cfg(unix)]
    fn platform_app_data_dir() -> PathBuf {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME should be set"));
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/ai.halv.app")
        } else {
            home.join(".local/share/ai.halv.app")
        }
    }

    // Resolves the symbolic links of the deepest existing ancestor, then adds
    // the missing components. Seatbelt and resolve_write_path use this form.
    // On macOS the default TMPDIR is under /var, a link to /private/var.
    #[cfg(unix)]
    fn canonical_path(path: &Path) -> PathBuf {
        for ancestor in path.ancestors() {
            if let Ok(mut canonical) = ancestor.canonicalize() {
                let rest = path
                    .strip_prefix(ancestor)
                    .expect("an ancestor should be a prefix of the path");
                canonical.extend(rest.components());
                return canonical;
            }
        }
        panic!("`{}` should have an existing ancestor", path.display());
    }

    #[cfg(unix)]
    #[test]
    fn sandbox_write_policy_refuses_session_files() {
        let project = std::env::temp_dir().join(format!("halv-session-project-{}", new_uuid_v4()));
        fs::create_dir_all(&project).expect("project directory should exist");
        let roots = WorkspaceRoots::for_cwd(&project);
        // The Seatbelt policy gives write access only to these subpaths.
        let writable_subpaths = crate::exec::workspace_write_seatbelt_policy(&roots)
            .lines()
            .filter_map(|line| {
                line.strip_prefix("(allow file-write* (subpath \"")?
                    .strip_suffix("\"))")
                    .map(PathBuf::from)
            })
            .collect::<Vec<_>>();
        assert!(!writable_subpaths.is_empty());
        let session = root_for(&platform_app_data_dir()).join(directory_name("session-123"));

        for path in [
            session.join(CODEX_HOME_DIRECTORY_NAME).join("config.toml"),
            session.join(CLAUDE_SETTINGS_FILE_NAME),
            session.join(KIMI_AGENT_FILE_NAME),
            session.join(KIMI_MCP_FILE_NAME),
        ] {
            let path = canonical_path(&path);
            // A resolve error is not a refusal. A path with a symbolic link
            // gives an error, but Seatbelt can still allow the write.
            let resolved = roots
                .resolve_write_path(&path)
                .unwrap_or_else(|error| panic!("`{}` should resolve: {error}", path.display()));
            assert!(
                !resolved.is_writable,
                "the sandbox must not write `{}`",
                path.display()
            );
            assert!(
                writable_subpaths
                    .iter()
                    .all(|subpath| !path.starts_with(subpath)),
                "the Seatbelt policy must not write `{}`",
                path.display()
            );
            // The read-only policy also gives write access to this directory.
            assert!(!path.starts_with(crate::exec::rtk_state_directory()));
        }

        // The temp directory, the old location, is a sandbox write root.
        let legacy = canonical_path(
            &std::env::temp_dir()
                .join("halv-codex-session-123")
                .join("config.toml"),
        );
        assert!(
            roots
                .resolve_write_path(&legacy)
                .expect("legacy path should resolve")
                .is_writable
        );
        assert!(writable_subpaths
            .iter()
            .any(|subpath| legacy.starts_with(subpath)));

        fs::remove_dir_all(project).expect("project directory should be removable");
    }

    // Model discovery runs `codex app-server` outside the sandbox. Codex can
    // load `.codex/config.toml` from its working directory.
    #[cfg(unix)]
    #[test]
    fn sandbox_write_policies_refuse_work_directories() {
        let project = std::env::temp_dir().join(format!("halv-work-project-{}", new_uuid_v4()));
        fs::create_dir_all(&project).expect("project directory should exist");
        let roots = WorkspaceRoots::for_cwd(&project);
        // Bubblewrap binds the RTK state directory only when it exists.
        let rtk_state_dir =
            canonical_path(&std::env::temp_dir().join(format!("halv-rtk-work-{}", new_uuid_v4())));
        fs::create_dir_all(&rtk_state_dir).expect("RTK state directory should exist");
        let writable = crate::exec::sandbox_write_paths(&roots, &rtk_state_dir);
        assert!(writable.contains(&rtk_state_dir));
        let root = root_for(&platform_app_data_dir());

        for purpose in ["models", "memory"] {
            let path = canonical_path(
                &root
                    .join(work_directory_name(purpose))
                    .join(".codex")
                    .join("config.toml"),
            );
            let resolved = roots
                .resolve_write_path(&path)
                .unwrap_or_else(|error| panic!("`{}` should resolve: {error}", path.display()));
            assert!(
                !resolved.is_writable,
                "the sandbox must not write `{}`",
                path.display()
            );
            assert!(
                writable.iter().all(|writable| !path.starts_with(writable)),
                "a sandbox policy must not write `{}`",
                path.display()
            );
        }

        // The temp directory, the old location, is a sandbox write root.
        let legacy = canonical_path(
            &std::env::temp_dir()
                .join("halv-models-123")
                .join(".codex")
                .join("config.toml"),
        );
        assert!(writable.iter().any(|writable| legacy.starts_with(writable)));

        fs::remove_dir_all(rtk_state_dir).expect("RTK state directory should be removable");
        fs::remove_dir_all(project).expect("project directory should be removable");
    }

    #[test]
    fn work_directory_is_private_and_drop_removes_it() {
        let first = WorkDirectory::create("models").expect("work directory should exist");
        let second = WorkDirectory::create("models").expect("work directory should exist");
        let path = first.path().to_path_buf();

        assert_ne!(path, second.path());
        assert_eq!(path.parent(), Some(root().unwrap().as_path()));
        assert!(path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("models-")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&path)
                .expect("work directory should have metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        fs::create_dir_all(path.join(".codex")).expect("work file directory should exist");

        drop(first);

        assert!(!path.exists());
        assert!(second.path().is_dir());
    }

    #[test]
    fn session_directory_is_private_and_removable() {
        let session_id = format!("session/{}", new_uuid_v4());

        let directory = directory(&session_id).expect("session directory should exist");

        assert_eq!(directory.parent(), Some(root().unwrap().as_path()));
        assert!(directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.contains('/')));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for path in [root().unwrap(), directory.clone()] {
                let mode = fs::metadata(&path)
                    .expect("session directory should have metadata")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o700, "`{}`", path.display());
            }
        }
        fs::write(directory.join(CLAUDE_SETTINGS_FILE_NAME), "{}")
            .expect("session file should write");

        remove(&session_id);

        assert!(!directory.exists());
    }

    #[test]
    fn stale_directories_keep_live_and_recent_sessions() {
        let root = std::env::temp_dir().join(format!("halv-session-stale-{}", new_uuid_v4()));
        for name in ["live-session", "stale-session"] {
            fs::create_dir_all(root.join(name)).expect("session directory should exist");
        }
        fs::write(root.join("stray-file"), "").expect("stray file should write");
        let live = HashSet::from([directory_name("live-session")]);
        let later = SystemTime::now() + Duration::from_secs(60);

        let stale = stale_directories(&root, &live, later);

        assert_eq!(stale, vec![root.join("stale-session")]);
        // A directory that changed after the cutoff stays.
        assert!(stale_directories(&root, &live, SystemTime::UNIX_EPOCH).is_empty());
        assert!(stale_directories(&root.join("missing"), &live, later).is_empty());
        fs::remove_dir_all(root).expect("test root should be removable");
    }
}
