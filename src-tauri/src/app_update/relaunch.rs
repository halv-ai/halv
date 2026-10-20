//! Starts the new Halv version after an install.

use std::{thread, time::Duration};
use tauri::AppHandle;

#[cfg(target_os = "macos")]
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

/// The time that Halv has to exit after an install. A stuck teardown must
/// not keep the old process alive, so the watchdog then ends it.
pub(super) const EXIT_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(target_os = "macos")]
const MACOS_RELAUNCH_SCRIPT: &str = r#"
parent_pid="$1"
bundle_path="$2"
executable_path="$3"
opener_path="$4"

while kill -0 "$parent_pid" 2>/dev/null; do
  sleep 0.1
done

"$opener_path" -n "$bundle_path" && exit 0
exec "$executable_path"
"#;

#[cfg(target_os = "macos")]
pub(super) fn macos_bundle_path(executable_path: &Path) -> Result<PathBuf, String> {
    let macos_directory = executable_path
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "MacOS"))
        .ok_or_else(|| {
            format!(
                "Halv is not running from a macOS app bundle: {}",
                executable_path.display()
            )
        })?;
    let contents_directory = macos_directory
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .ok_or_else(|| {
            format!(
                "Halv is not running from a macOS app bundle: {}",
                executable_path.display()
            )
        })?;
    let bundle_path = contents_directory.parent().filter(|path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    });
    bundle_path.map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "Halv is not running from a macOS app bundle: {}",
            executable_path.display()
        )
    })
}

#[cfg(target_os = "macos")]
fn spawn_macos_relaunch_helper_with_opener(
    executable_path: &Path,
    parent_pid: u32,
    opener_path: &Path,
) -> Result<Child, String> {
    let bundle_path = macos_bundle_path(executable_path)?;
    Command::new("/bin/sh")
        .arg("-c")
        .arg(MACOS_RELAUNCH_SCRIPT)
        .arg("halv-update-relaunch")
        .arg(parent_pid.to_string())
        .arg(bundle_path)
        .arg(executable_path)
        .arg(opener_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to schedule Halv relaunch: {error}"))
}

#[cfg(target_os = "macos")]
fn spawn_macos_relaunch_helper(executable_path: &Path, parent_pid: u32) -> Result<(), String> {
    spawn_macos_relaunch_helper_with_opener(executable_path, parent_pid, Path::new("/usr/bin/open"))
        .map(|_| ())
}

/// Ends Halv and starts the installed version. The Windows installer starts
/// Halv itself, so this path runs only on macOS and Linux.
pub(super) fn relaunch(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let executable_path = std::env::current_exe()
            .map_err(|error| format!("failed to locate Halv after the update: {error}"))?;
        spawn_macos_relaunch_helper(&executable_path, std::process::id())?;
        // The helper waits for this process, so a forced exit still relaunches.
        arm_exit_watchdog(EXIT_WATCHDOG_TIMEOUT, || std::process::exit(0));
        app.exit(0);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let env = tauri::Manager::env(app);
        arm_exit_watchdog(EXIT_WATCHDOG_TIMEOUT, move || {
            tauri::process::restart(&env);
        });
        app.request_restart();
        Ok(())
    }
}

/// Runs `on_timeout` when the process still lives after `timeout`.
pub(super) fn arm_exit_watchdog(timeout: Duration, on_timeout: impl FnOnce() + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("halv-update-exit-watchdog".to_string())
        .spawn(move || {
            thread::sleep(timeout);
            eprintln!(
                "Halv did not exit within {} seconds after the update; forcing the exit",
                timeout.as_secs()
            );
            on_timeout();
        });
    if let Err(error) = spawned {
        eprintln!("failed to start the update exit watchdog: {error}");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{macos_bundle_path, spawn_macos_relaunch_helper_with_opener};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    fn test_directory() -> PathBuf {
        // The macOS clock counts microseconds, so two tests can read the same
        // time. The counter keeps their directories apart.
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must follow the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "halv-update-relaunch-{}-{nonce}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("test directory should be created");
        path
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).expect("test executable should be written");
        let mut permissions = fs::metadata(path)
            .expect("test executable metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("test executable should be executable");
    }

    fn wait_for_child(child: &mut Child) {
        // The helper usually exits in less than 1 second. A busy machine can
        // start its shell and its `sleep` processes much later.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if child
                .try_wait()
                .expect("relaunch helper status should load")
                .is_some()
            {
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("relaunch helper did not exit");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn finds_the_bundle_from_the_installed_executable() {
        assert_eq!(
            macos_bundle_path(Path::new("/Applications/Halv.app/Contents/MacOS/halv")),
            Ok(PathBuf::from("/Applications/Halv.app"))
        );
    }

    #[test]
    fn rejects_an_executable_outside_a_bundle() {
        let error = macos_bundle_path(Path::new("/usr/local/bin/halv"))
            .expect_err("a standalone executable must not schedule a bundle relaunch");

        assert!(error.contains("not running from a macOS app bundle"));
    }

    #[test]
    fn helper_waits_for_exit_and_preserves_special_paths() {
        let root = test_directory();
        let bundle = root.join("Halv O'Brien.app");
        let executable = bundle.join("Contents/MacOS/halv");
        fs::create_dir_all(executable.parent().expect("executable must have a parent"))
            .expect("test bundle should be created");
        write_executable(&executable, "#!/bin/sh\nexit 0\n");

        let opener = root.join("fake opener");
        let opener_log = root.join("fake opener.log");
        write_executable(
            &opener,
            "#!/bin/sh\nprintf '%s\\n' \"$1\" \"$2\" > \"$0.log\"\n",
        );
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "while :; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("test parent should start");
        let mut helper = spawn_macos_relaunch_helper_with_opener(&executable, parent.id(), &opener)
            .expect("relaunch helper should start");

        thread::sleep(Duration::from_millis(250));
        assert!(!opener_log.exists(), "opener must wait for the app to exit");

        parent.kill().expect("test parent should stop");
        parent.wait().expect("test parent should be reaped");
        wait_for_child(&mut helper);

        let arguments = fs::read_to_string(opener_log).expect("opener arguments should be logged");
        assert_eq!(arguments, format!("-n\n{}\n", bundle.display()));
        fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn helper_runs_the_updated_executable_when_open_fails() {
        let root = test_directory();
        let bundle = root.join("Halv Test.app");
        let executable = bundle.join("Contents/MacOS/halv");
        let fallback_log = bundle.join("Contents/MacOS/halv.fallback");
        fs::create_dir_all(executable.parent().expect("executable must have a parent"))
            .expect("test bundle should be created");
        write_executable(
            &executable,
            "#!/bin/sh\nprintf 'fallback\\n' > \"$0.fallback\"\n",
        );

        let opener = root.join("failing opener");
        write_executable(&opener, "#!/bin/sh\nexit 1\n");
        let mut exited_parent = Command::new("/usr/bin/true")
            .spawn()
            .expect("test parent should start");
        let parent_pid = exited_parent.id();
        exited_parent.wait().expect("test parent should exit");
        let mut helper = spawn_macos_relaunch_helper_with_opener(&executable, parent_pid, &opener)
            .expect("relaunch helper should start");

        wait_for_child(&mut helper);
        assert_eq!(
            fs::read_to_string(fallback_log).expect("fallback should run"),
            "fallback\n"
        );
        fs::remove_dir_all(root).expect("test directory should be removed");
    }
}
