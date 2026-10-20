//! Child process helpers that behave the same on every platform.
//!
//! The release build on Windows is a GUI-subsystem program. A GUI parent
//! has no console. Thus each console child that starts without
//! `CREATE_NO_WINDOW` allocates its own visible console window. The helpers
//! here set that flag on Windows and do nothing on the other platforms.
//!
//! Do not use these helpers for a PTY spawn. ConPTY needs a console, and
//! `CREATE_NO_WINDOW` breaks it. Do not use them for a program that must
//! show a window, for example a browser or an editor started through
//! `start` or `open`.

#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// Starts the child without a console window on Windows.
///
/// The call is a no-op on the other platforms.
pub(crate) fn hide_console(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Starts the asynchronous child without a console window on Windows.
///
/// The call is a no-op on the other platforms.
pub(crate) fn hide_console_tokio(
    command: &mut tokio::process::Command,
) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hide_console_keeps_program_and_args() {
        let mut command = std::process::Command::new("git");
        command.arg("--version");
        let same = hide_console(&mut command);
        assert_eq!(same.get_program(), "git");
        let args = same
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--version"]);
    }

    #[test]
    fn hide_console_tokio_keeps_program_and_args() {
        let mut command = tokio::process::Command::new("git");
        command.arg("--version");
        let same = hide_console_tokio(&mut command);
        let std_command = same.as_std();
        assert_eq!(std_command.get_program(), "git");
        let args = std_command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, ["--version"]);
    }

    /// The helper is a no-op off Windows: the debug output of the command
    /// stays the same as for an untouched command.
    #[cfg(not(windows))]
    #[test]
    fn hide_console_is_a_no_op_off_windows() {
        let mut plain = std::process::Command::new("git");
        plain.arg("--version");
        let mut hidden = std::process::Command::new("git");
        hidden.arg("--version");
        hide_console(&mut hidden);
        assert_eq!(format!("{plain:?}"), format!("{hidden:?}"));
    }

    /// Windows keeps the flag value that `wallet/files.rs` used before the
    /// helper existed.
    #[cfg(windows)]
    #[test]
    fn create_no_window_matches_the_win32_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
