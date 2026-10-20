use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tauri::{AppHandle, Manager, Runtime};

const FRESH_STATE_ENV: &str = "VITE_HALV_FRESH_STATE";

static RESOLVED_APP_DATA_DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AppDataDirDecision {
    pub(crate) path: PathBuf,
    pub(crate) wipe: bool,
}

pub(crate) fn app_data_dir_decision(
    base: &Path,
    fresh_state_env: Option<&str>,
    debug_build: bool,
) -> Result<AppDataDirDecision, String> {
    if !debug_build || fresh_state_env != Some("1") {
        return Ok(AppDataDirDecision {
            path: base.to_path_buf(),
            wipe: false,
        });
    }

    let Some(file_name) = base.file_name() else {
        return Err("Halv could not create the fresh app data directory name.".to_string());
    };
    let mut fresh_file_name = OsString::from(file_name);
    fresh_file_name.push("-fresh");

    Ok(AppDataDirDecision {
        path: base.with_file_name(fresh_file_name),
        wipe: true,
    })
}

pub(crate) fn resolved_app_data_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    RESOLVED_APP_DATA_DIR
        .get_or_init(|| {
            let base = app.path().app_data_dir().map_err(|error| {
                format!("Halv could not locate its app data directory: {error}")
            })?;
            let fresh_state_env = std::env::var(FRESH_STATE_ENV).ok();
            let decision =
                app_data_dir_decision(&base, fresh_state_env.as_deref(), cfg!(debug_assertions))?;
            if decision.wipe {
                reset_fresh_app_data_dir(&decision.path).map_err(|error| {
                    format!(
                        "Halv could not reset the fresh app data directory at {}: {error}",
                        decision.path.display()
                    )
                })?;
            }
            Ok(decision.path)
        })
        .clone()
}

/// Returns the app data directory after `resolved_app_data_dir` resolved it.
/// Before setup, or after a failed resolution, it returns `None`.
#[cfg(target_os = "windows")]
pub(crate) fn cached_app_data_dir() -> Option<PathBuf> {
    RESOLVED_APP_DATA_DIR
        .get()
        .and_then(|resolved| resolved.as_ref().ok().cloned())
}

fn reset_fresh_app_data_dir(path: &Path) -> io::Result<()> {
    if !wipe_allowed(path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Halv refused to wipe an app data directory without the -fresh suffix.",
        ));
    }

    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir_all(path)
}

fn wipe_allowed(path: &Path) -> bool {
    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .is_some_and(|file_name| file_name.ends_with("-fresh"))
}

/// Creates a directory and its parents, then restricts the leaf to its owner.
///
/// The app data directory holds the Halv session token, the terminal
/// scrollback, and MCP secrets. A failure to create the directory is an
/// error. A failure to change its mode is only a warning.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    warn_on_mode_error(path, restrict_dir(path));
    Ok(())
}

/// Creates an empty owner-only file when `path` is missing. An existing file
/// keeps its content and gets owner-only access through `restrict_file`.
pub(crate) fn create_private_file(path: &Path) -> io::Result<()> {
    match private_file_options()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            restrict_file(path);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Returns options that create a file with owner-only access.
#[cfg(unix)]
pub(crate) fn private_file_options() -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}

#[cfg(not(unix))]
pub(crate) fn private_file_options() -> OpenOptions {
    OpenOptions::new()
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Restricts an existing file to its owner at startup. A missing file is not
/// an error.
///
/// Some file systems, for example FAT or FUSE, reject a mode change. Halv
/// then logs a warning and continues, because the data stays usable.
pub(crate) fn restrict_file(path: &Path) {
    warn_on_mode_error(path, restrict_file_mode(path));
}

#[cfg(unix)]
fn restrict_file_mode(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(not(unix))]
fn restrict_file_mode(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn warn_on_mode_error(path: &Path, result: io::Result<()>) {
    if let Some(warning) = mode_change_warning(path, result) {
        eprintln!("{warning}");
    }
}

/// Returns the warning for a failed mode change, or `None` when the mode
/// change succeeded.
fn mode_change_warning(path: &Path, result: io::Result<()>) -> Option<String> {
    result.err().map(|error| {
        format!(
            "app data: could not restrict {} to its owner: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_off_keeps_base_path_without_wipe() {
        let base = Path::new("/tmp/com.halv.app");

        let decision = app_data_dir_decision(base, None, true).expect("path should resolve");

        assert_eq!(decision.path, base);
        assert!(!decision.wipe);
    }

    #[test]
    fn flag_on_in_debug_uses_fresh_sibling_with_wipe() {
        let base = Path::new("/tmp/com.halv.app");

        let decision = app_data_dir_decision(base, Some("1"), true).expect("path should resolve");

        assert_eq!(decision.path, Path::new("/tmp/com.halv.app-fresh"));
        assert!(decision.wipe);
    }

    #[test]
    fn wipe_guard_refuses_path_without_fresh_suffix() {
        let path = std::env::temp_dir().join(format!("halv-app-data-guard-{}", std::process::id()));
        fs::create_dir_all(&path).expect("test directory should exist");
        let marker = path.join("marker");
        fs::write(&marker, "keep").expect("test marker should exist");

        let error = reset_fresh_app_data_dir(&path).expect_err("wipe should be refused");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(marker.exists());
        fs::remove_dir_all(path).expect("test directory should be removable");
    }

    #[cfg(unix)]
    #[test]
    fn private_paths_are_owner_only_and_keep_existing_content() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| {
            fs::metadata(path)
                .expect("path should exist")
                .permissions()
                .mode()
                & 0o777
        };
        let root =
            std::env::temp_dir().join(format!("halv-app-data-private-{}", rand::random::<u64>()));
        let directory = root.join("data");
        create_private_dir(&directory).expect("directory should be created");
        assert_eq!(mode(&directory), 0o700);

        let new_file = directory.join("new");
        create_private_file(&new_file).expect("file should be created");
        assert_eq!(mode(&new_file), 0o600);

        let old_file = directory.join("old");
        fs::write(&old_file, "keep").expect("file should be written");
        fs::set_permissions(&old_file, fs::Permissions::from_mode(0o644))
            .expect("mode should change");
        create_private_file(&old_file).expect("existing file should be kept");
        assert_eq!(mode(&old_file), 0o600);
        assert_eq!(
            fs::read_to_string(&old_file).expect("file should read"),
            "keep"
        );

        restrict_file(&directory.join("missing"));
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[test]
    fn a_failed_mode_change_gives_a_warning() {
        let path = Path::new("/data/halv.sqlite3");
        assert_eq!(mode_change_warning(path, Ok(())), None);

        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Unsupported] {
            let warning = mode_change_warning(path, Err(io::Error::from(kind)))
                .expect("a failed mode change should give a warning");
            assert!(warning.contains("/data/halv.sqlite3"), "{warning}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_cannot_be_created_is_still_an_error() {
        let root =
            std::env::temp_dir().join(format!("halv-app-data-create-{}", rand::random::<u64>()));
        create_private_dir(&root).expect("directory should be created");
        let blocker = root.join("file");
        create_private_file(&blocker).expect("file should be created");

        create_private_dir(&blocker.join("data"))
            .expect_err("a directory under a file should fail");
        create_private_file(&root.join("missing").join("halv.sqlite3"))
            .expect_err("a file in a missing directory should fail");
        fs::remove_dir_all(root).expect("test directory should be removable");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_rejected_mode_change_does_not_stop_startup() {
        use super::test_support::ImmutableFlag;
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| {
            fs::metadata(path)
                .expect("path should exist")
                .permissions()
                .mode()
                & 0o777
        };
        let root =
            std::env::temp_dir().join(format!("halv-app-data-rejected-{}", rand::random::<u64>()));
        let directory = root.join("data");
        fs::create_dir_all(&directory).expect("directory should be created");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .expect("mode should change");
        let file = root.join("old");
        fs::write(&file, "keep").expect("file should be written");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("mode should change");

        {
            let _directory_flag = ImmutableFlag::set(&directory);
            let _file_flag = ImmutableFlag::set(&file);
            create_private_dir(&directory).expect("a rejected directory mode should not fail");
            create_private_file(&file).expect("a rejected file mode should not fail");
            restrict_file(&file);
            assert_eq!(mode(&directory), 0o755);
            assert_eq!(mode(&file), 0o644);
        }

        // A new file still gets owner-only access.
        let new_file = root.join("new");
        create_private_file(&new_file).expect("file should be created");
        assert_eq!(mode(&new_file), 0o600);
        fs::remove_dir_all(root).expect("test directory should be removable");
    }
}

/// Test helpers that make the kernel reject a mode change.
#[cfg(all(test, target_os = "macos"))]
pub(crate) mod test_support {
    use std::{
        path::{Path, PathBuf},
        process::Command,
    };

    /// Sets the user immutable flag on a path until the value drops. The
    /// kernel then rejects a mode change, as a FAT or FUSE file system can.
    pub(crate) struct ImmutableFlag(PathBuf);

    impl ImmutableFlag {
        pub(crate) fn set(path: &Path) -> Self {
            let status = Command::new("chflags")
                .arg("uchg")
                .arg(path)
                .status()
                .expect("chflags should run");
            assert!(
                status.success(),
                "chflags uchg failed for {}",
                path.display()
            );
            Self(path.to_path_buf())
        }
    }

    impl Drop for ImmutableFlag {
        fn drop(&mut self) {
            let _ = Command::new("chflags").arg("nouchg").arg(&self.0).status();
        }
    }
}
