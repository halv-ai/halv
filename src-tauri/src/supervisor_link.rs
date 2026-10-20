use halv_supervisor::SupervisorClient;
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SUPERVISOR_BINARY_ENV: &str = "HALV_SUPERVISOR_BINARY";
const START_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn socket_path(app_data_dir: &Path) -> PathBuf {
    socket_path_for_profile(app_data_dir, cfg!(debug_assertions))
}

pub(crate) fn log_path(app_data_dir: &Path) -> PathBuf {
    log_path_for_profile(app_data_dir, cfg!(debug_assertions))
}

fn socket_path_for_profile(app_data_dir: &Path, debug: bool) -> PathBuf {
    app_data_dir.join(if debug {
        "supervisor-dev.sock"
    } else {
        "supervisor.sock"
    })
}

fn log_path_for_profile(app_data_dir: &Path, debug: bool) -> PathBuf {
    app_data_dir.join(if debug {
        "supervisor-dev.log"
    } else {
        "supervisor.log"
    })
}

pub(crate) fn locate_supervisor_binary() -> Result<PathBuf, String> {
    locate_supervisor_binary_with(
        std::env::var_os(SUPERVISOR_BINARY_ENV),
        std::env::current_exe().ok(),
        Path::new(env!("CARGO_MANIFEST_DIR")),
        std::env::var_os("PATH"),
        is_executable_file,
    )
    .ok_or_else(|| "failed to locate halv-supervisor binary".to_string())
}

fn locate_supervisor_binary_with(
    explicit_override: Option<OsString>,
    current_exe: Option<PathBuf>,
    manifest_dir: &Path,
    path: Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    explicit_override
        .map(PathBuf::from)
        .filter(|candidate| exists(candidate))
        .or_else(|| {
            current_exe
                .and_then(|path| path.parent().map(|parent| parent.join("halv-supervisor")))
                .filter(|candidate| exists(candidate))
        })
        .or_else(|| {
            let candidate = manifest_dir.join("supervisor/target/release/halv-supervisor");
            exists(&candidate).then_some(candidate)
        })
        .or_else(|| {
            let candidate = manifest_dir.join("supervisor/target/debug/halv-supervisor");
            exists(&candidate).then_some(candidate)
        })
        .or_else(|| find_on_path(path.as_deref(), "halv-supervisor", &exists))
}

fn find_on_path(
    path: Option<&OsStr>,
    binary_name: &str,
    exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(binary_name))
        .find(|candidate| exists(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub(crate) fn ensure_running(
    app_data_dir: &Path,
    app_version: &str,
) -> Result<SupervisorClient, String> {
    let socket = socket_path(app_data_dir);
    match SupervisorClient::connect(&socket, app_version) {
        Ok(client) => return Ok(client),
        Err(error) if is_protocol_mismatch(&error) => {
            eprintln!("{error}");
            return Err(error);
        }
        Err(_) => {}
    }

    fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("failed to create supervisor data directory: {error}"))?;
    let binary = locate_supervisor_binary()?;
    let log = log_path(app_data_dir);
    let mut command = Command::new(&binary);
    command
        .arg("--socket")
        .arg(&socket)
        .arg("--log")
        .arg(&log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|error| {
        format!(
            "failed to start supervisor binary `{}`: {error}",
            binary.display()
        )
    })?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let error = match SupervisorClient::connect(&socket, app_version) {
            Ok(client) => {
                spawn_supervisor_reaper(child);
                return Ok(client);
            }
            Err(error) if is_protocol_mismatch(&error) => {
                eprintln!("{error}");
                spawn_supervisor_reaper(child);
                return Err(error);
            }
            Err(error) => error,
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "failed to start supervisor: process exited with {status}"
                ));
            }
            Ok(None) => {}
            Err(error) => {
                spawn_supervisor_reaper(child);
                return Err(format!("failed to inspect supervisor process: {error}"));
            }
        }
        if Instant::now() >= deadline {
            spawn_supervisor_reaper(child);
            return Err(format!(
                "failed to connect to supervisor within 3 seconds: {error}"
            ));
        }
        thread::sleep(CONNECT_POLL_INTERVAL);
    }
}

pub(crate) fn wait_for_socket_release(app_data_dir: &Path) -> Result<(), String> {
    let socket = socket_path(app_data_dir);
    wait_for_socket_release_with(
        &socket,
        SHUTDOWN_TIMEOUT,
        |path| !path.exists(),
        thread::sleep,
    )
}

fn wait_for_socket_release_with(
    socket: &Path,
    timeout: Duration,
    mut released: impl FnMut(&Path) -> bool,
    mut wait: impl FnMut(Duration),
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if released(socket) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "terminal supervisor did not release socket `{}` within {} seconds",
                socket.display(),
                timeout.as_secs()
            ));
        }
        wait(CONNECT_POLL_INTERVAL);
    }
}

fn spawn_supervisor_reaper(mut child: Child) {
    if let Err(error) = thread::Builder::new()
        .name("halv-supervisor-reaper".to_string())
        .spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("failed to reap supervisor process: {error}");
            }
        })
    {
        eprintln!("failed to start supervisor reaper: {error}");
    }
}

/// Connects to a running supervisor. `Ok(None)` means that no supervisor
/// answers. An error means that a supervisor answers with another protocol.
pub(crate) fn connect_existing(
    app_data_dir: &Path,
    app_version: &str,
) -> Result<Option<SupervisorClient>, String> {
    let socket = socket_path(app_data_dir);
    match SupervisorClient::connect(socket, app_version) {
        Ok(client) => Ok(Some(client)),
        Err(error) if is_protocol_mismatch(&error) => {
            eprintln!("{error}");
            Err(error)
        }
        Err(_) => Ok(None),
    }
}

pub(crate) fn is_broken_connection(error: &str) -> bool {
    error.contains("supervisor socket")
        || error.contains("supervisor request")
        || error.contains("supervisor response")
        || error.contains("connection closed")
        || error.contains("Broken pipe")
        || error.contains("Connection reset")
}

pub(crate) fn is_protocol_mismatch(error: &str) -> bool {
    error.to_ascii_lowercase().contains("protocol")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn supervisor_paths_are_separate_by_build_profile() {
        let app_data_dir = Path::new("/app-data");
        let debug_socket = socket_path_for_profile(app_data_dir, true);
        let release_socket = socket_path_for_profile(app_data_dir, false);

        assert_eq!(debug_socket, app_data_dir.join("supervisor-dev.sock"));
        assert_eq!(release_socket, app_data_dir.join("supervisor.sock"));
        assert_ne!(debug_socket, release_socket);
        assert_eq!(
            log_path_for_profile(app_data_dir, true),
            app_data_dir.join("supervisor-dev.log")
        );
        assert_eq!(
            log_path_for_profile(app_data_dir, false),
            app_data_dir.join("supervisor.log")
        );
    }

    #[test]
    fn locator_uses_the_documented_order() {
        let manifest = Path::new("/manifest");
        let explicit = PathBuf::from("/override/halv-supervisor");
        let sidecar = PathBuf::from("/app/halv-supervisor");
        let release = manifest.join("supervisor/target/release/halv-supervisor");
        let debug = manifest.join("supervisor/target/debug/halv-supervisor");
        let path_binary = PathBuf::from("/path-bin/halv-supervisor");
        let mut available = HashSet::from([
            explicit.clone(),
            sidecar.clone(),
            release.clone(),
            debug.clone(),
            path_binary.clone(),
        ]);
        let path = std::env::join_paths([Path::new("/path-bin")]).unwrap();
        let locate = |available: &HashSet<PathBuf>| {
            locate_supervisor_binary_with(
                Some(explicit.clone().into_os_string()),
                Some(PathBuf::from("/app/Halv")),
                manifest,
                Some(path.clone()),
                |candidate| available.contains(candidate),
            )
        };

        assert_eq!(locate(&available), Some(explicit.clone()));
        available.remove(&explicit);
        assert_eq!(locate(&available), Some(sidecar.clone()));
        available.remove(&sidecar);
        assert_eq!(locate(&available), Some(release.clone()));
        available.remove(&release);
        assert_eq!(locate(&available), Some(debug.clone()));
        available.remove(&debug);
        assert_eq!(locate(&available), Some(path_binary));
    }

    #[test]
    fn socket_release_timeout_returns_a_clear_error() {
        let socket = Path::new("/app-data/supervisor.sock");
        let error = wait_for_socket_release_with(
            socket,
            Duration::ZERO,
            |_| false,
            |_| panic!("a zero timeout must not sleep"),
        )
        .expect_err("an occupied socket should time out");

        assert_eq!(
            error,
            "terminal supervisor did not release socket `/app-data/supervisor.sock` within 0 seconds"
        );
    }
}
