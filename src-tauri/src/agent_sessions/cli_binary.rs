//! Shared rules for finding an agent CLI binary on the host.
//!
//! npm puts a POSIX shell script `codex` next to `codex.cmd` in
//! `%APPDATA%\npm`. Windows cannot run the shell script. The Windows
//! candidate order puts the executable extensions first, and the Windows
//! executable check rejects files with no executable extension.
//!
//! A `.cmd` npm shim starts `node` on a script under `node_modules`. Halv
//! resolves the shim so the child it holds is the real process, not a
//! `cmd.exe` wrapper.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostOs {
    Unix,
    Windows,
}

impl HostOs {
    pub(super) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Extensions Windows runs directly. `PATHEXT` can list more, but these
/// cover every agent CLI install that Halv knows.
const WINDOWS_EXECUTABLE_EXTENSIONS: [&str; 4] = ["exe", "cmd", "bat", "com"];

/// Extensions the candidate list tries before the bare name on Windows.
const WINDOWS_CANDIDATE_EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// How deep the native binary search walks below a package directory.
const NATIVE_BINARY_MAX_DEPTH: usize = 6;

/// Adds the candidate paths for `name` inside `directory`.
///
/// Windows tries `.exe`, `.cmd`, and `.bat` before the bare name. Unix
/// tries only the bare name.
pub(super) fn push_candidates(
    candidates: &mut Vec<PathBuf>,
    directory: &Path,
    name: &str,
    os: HostOs,
) {
    if os == HostOs::Windows {
        for extension in WINDOWS_CANDIDATE_EXTENSIONS {
            candidates.push(directory.join(format!("{name}.{extension}")));
        }
    }
    candidates.push(directory.join(name));
}

/// Reports whether the host can run `path` as a program.
pub(super) fn is_executable(path: &Path) -> bool {
    is_executable_on(path, HostOs::current())
}

/// Reports whether `os` can run `path` as a program.
///
/// Windows accepts only regular files with an executable extension. Unix
/// accepts regular files with an execute bit.
pub(super) fn is_executable_on(path: &Path, os: HostOs) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    match os {
        HostOs::Windows => has_extension_in(path, &WINDOWS_EXECUTABLE_EXTENSIONS),
        HostOs::Unix => unix_mode_allows_execution(&metadata),
    }
}

fn has_extension_in(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

#[cfg(unix)]
fn unix_mode_allows_execution(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn unix_mode_allows_execution(_metadata: &fs::Metadata) -> bool {
    true
}

/// The program Halv spawns plus the arguments that go before the CLI's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Launch {
    fn direct(program: &Path) -> Self {
        Self {
            program: program.to_path_buf(),
            args: Vec::new(),
        }
    }

    /// Appends the CLI's own arguments after the launch prefix.
    pub(super) fn with_args(mut self, args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
        self.args.extend(args);
        self.args
    }
}

/// Resolves `binary` to the process Halv should hold.
///
/// An npm `.cmd` or `.bat` shim resolves to the native binary it wraps, or
/// to `node` plus the script. Any other path launches as it is. A shim
/// that Halv cannot read launches as it is too; Rust runs it through
/// `cmd.exe`.
pub(super) fn launch_for(binary: &Path) -> Launch {
    resolve_npm_shim(binary).unwrap_or_else(|| Launch::direct(binary))
}

/// Builds a `Command` for `binary` with the launch prefix applied.
pub(super) fn command_for(binary: &Path) -> Command {
    let launch = launch_for(binary);
    let mut command = Command::new(launch.program);
    command.args(launch.args);
    crate::process::hide_console(&mut command);
    command
}

fn resolve_npm_shim(shim: &Path) -> Option<Launch> {
    if !has_extension_in(shim, &["cmd", "bat"]) {
        return None;
    }
    let shim_directory = shim.parent()?;
    let text = fs::read_to_string(shim).ok()?;
    let script = npm_shim_script(&text, shim_directory)?;
    if !script.is_file() {
        return None;
    }
    let name = shim.file_stem()?.to_str()?;
    if let Some(native) = native_binary_near(&script, name) {
        return Some(Launch::direct(&native));
    }
    Some(Launch {
        program: node_program(shim_directory),
        args: vec![script.into_os_string()],
    })
}

/// Finds the `"%dp0%\node_modules\...\<name>.js"` token in an npm shim.
///
/// The shim is a `cmd.exe` script, so the token uses backslashes. The
/// function splits on both separators so the test suite can run on macOS.
pub(super) fn npm_shim_script(text: &str, shim_directory: &Path) -> Option<PathBuf> {
    text.split('"')
        .filter_map(|token| token.strip_prefix("%dp0%"))
        .find(|rest| rest.contains("node_modules"))
        .map(|rest| {
            let relative: PathBuf = rest
                .split(['\\', '/'])
                .filter(|component| !component.is_empty())
                .collect();
            shim_directory.join(relative)
        })
}

/// Picks the `node` that runs the script.
///
/// npm shims prefer `node.exe` next to the shim. Halv does the same, then
/// looks on PATH, then leaves the lookup to the spawn.
fn node_program(shim_directory: &Path) -> PathBuf {
    let next_to_shim = shim_directory.join("node.exe");
    if next_to_shim.is_file() {
        return next_to_shim;
    }
    env::var_os("PATH")
        .into_iter()
        .flat_map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join("node.exe"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("node"))
}

/// Finds `<name>.exe` that the package ships for the host.
///
/// The script lives at `<package>/bin/<name>.js`. The platform package
/// sits under `<package>/node_modules`, next to `<package>` in the scope
/// directory, or inside `<package>/vendor`.
fn native_binary_near(script: &Path, name: &str) -> Option<PathBuf> {
    let package = script.parent()?.parent()?;
    let file_name = format!("{name}.exe");
    let roots = [
        package.join("vendor"),
        package.join("node_modules"),
        package.parent()?.to_path_buf(),
    ];
    let mut matches = Vec::new();
    for root in roots {
        collect_files_named(&root, &file_name, 0, &mut matches);
    }
    matches.sort();
    let arch_token = match env::consts::ARCH {
        "aarch64" => "aarch64",
        _ => "x86_64",
    };
    matches
        .iter()
        .find(|path| path.to_string_lossy().contains(arch_token))
        .or_else(|| matches.first())
        .cloned()
}

fn collect_files_named(directory: &Path, file_name: &str, depth: usize, found: &mut Vec<PathBuf>) {
    if depth >= NATIVE_BINARY_MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_files_named(&path, file_name, depth + 1, found);
        } else if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(file_name)
        {
            found.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    const NPM_SHIM: &str = concat!(
        "@ECHO off\r\n",
        "GOTO start\r\n",
        ":find_dp0\r\n",
        "SET dp0=%~dp0\r\n",
        "EXIT /b\r\n",
        ":start\r\n",
        "SETLOCAL\r\n",
        "CALL :find_dp0\r\n",
        "\r\n",
        "IF EXIST \"%dp0%\\node.exe\" (\r\n",
        "  SET \"_prog=%dp0%\\node.exe\"\r\n",
        ") ELSE (\r\n",
        "  SET \"_prog=node\"\r\n",
        "  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n",
        ")\r\n",
        "\r\n",
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & ",
        "\"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n",
    );

    fn temp_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            env::temp_dir().join(format!("halv-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Mirrors `%APPDATA%\npm` after `npm install -g @openai/codex`.
    fn npm_prefix(label: &str) -> PathBuf {
        let prefix = temp_directory(label);
        write_file(
            &prefix.join("codex"),
            "#!/bin/sh\nexec node codex.js \"$@\"\n",
        );
        write_file(&prefix.join("codex.cmd"), NPM_SHIM);
        write_file(
            &prefix.join("node_modules/@openai/codex/bin/codex.js"),
            "#!/usr/bin/env node\n",
        );
        prefix
    }

    fn first_executable(candidates: &[PathBuf], os: HostOs) -> Option<PathBuf> {
        candidates
            .iter()
            .find(|candidate| is_executable_on(candidate, os))
            .cloned()
    }

    #[test]
    fn windows_tries_executable_extensions_before_the_bare_name() {
        let mut candidates = Vec::new();
        push_candidates(&mut candidates, Path::new("bin"), "codex", HostOs::Windows);
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("bin/codex.exe"),
                PathBuf::from("bin/codex.cmd"),
                PathBuf::from("bin/codex.bat"),
                PathBuf::from("bin/codex"),
            ]
        );
    }

    #[test]
    fn unix_tries_only_the_bare_name() {
        let mut candidates = Vec::new();
        push_candidates(&mut candidates, Path::new("bin"), "codex", HostOs::Unix);
        assert_eq!(candidates, vec![PathBuf::from("bin/codex")]);
    }

    #[test]
    fn windows_picks_the_cmd_shim_over_the_npm_shell_script() {
        let prefix = npm_prefix("npm-windows");
        let mut candidates = Vec::new();
        push_candidates(&mut candidates, &prefix, "codex", HostOs::Windows);
        assert_eq!(
            first_executable(&candidates, HostOs::Windows),
            Some(prefix.join("codex.cmd"))
        );
        assert!(!is_executable_on(&prefix.join("codex"), HostOs::Windows));
        fs::remove_dir_all(prefix).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_picks_the_bare_name_over_the_cmd_shim() {
        let prefix = npm_prefix("npm-unix");
        let mut candidates = Vec::new();
        push_candidates(&mut candidates, &prefix, "codex", HostOs::Unix);
        assert_eq!(
            first_executable(&candidates, HostOs::Unix),
            Some(prefix.join("codex"))
        );
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn windows_rejects_directories_and_unknown_extensions() {
        let prefix = temp_directory("npm-extensions");
        fs::create_dir_all(prefix.join("codex.exe")).unwrap();
        write_file(&prefix.join("codex.ps1"), "");
        write_file(&prefix.join("codex.EXE.bak"), "");
        write_file(&prefix.join("real.EXE"), "");
        assert!(!is_executable_on(
            &prefix.join("codex.exe"),
            HostOs::Windows
        ));
        assert!(!is_executable_on(
            &prefix.join("codex.ps1"),
            HostOs::Windows
        ));
        assert!(!is_executable_on(
            &prefix.join("codex.EXE.bak"),
            HostOs::Windows
        ));
        assert!(is_executable_on(&prefix.join("real.EXE"), HostOs::Windows));
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn parses_the_script_path_out_of_an_npm_shim() {
        let script = npm_shim_script(NPM_SHIM, Path::new("prefix")).unwrap();
        assert_eq!(
            script,
            Path::new("prefix")
                .join("node_modules")
                .join("@openai")
                .join("codex")
                .join("bin")
                .join("codex.js")
        );
        assert_eq!(npm_shim_script("@ECHO off\r\n", Path::new("prefix")), None);
    }

    #[test]
    fn a_cmd_shim_launches_node_on_the_script() {
        let prefix = npm_prefix("npm-shim-node");
        let launch = launch_for(&prefix.join("codex.cmd"));
        let script = prefix.join("node_modules/@openai/codex/bin/codex.js");
        assert_eq!(launch.args, vec![script.clone().into_os_string()]);
        assert_eq!(
            launch.program.file_stem().and_then(|stem| stem.to_str()),
            Some("node")
        );
        assert_eq!(
            launch.clone().with_args([OsString::from("--version")]),
            vec![script.into_os_string(), OsString::from("--version")]
        );
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn a_cmd_shim_prefers_node_exe_next_to_it() {
        let prefix = npm_prefix("npm-shim-node-exe");
        write_file(&prefix.join("node.exe"), "");
        let launch = launch_for(&prefix.join("codex.cmd"));
        assert_eq!(launch.program, prefix.join("node.exe"));
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn a_cmd_shim_resolves_to_the_platform_binary() {
        let prefix = npm_prefix("npm-shim-native");
        let native = prefix.join(
            "node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
        );
        write_file(&native, "");
        let launch = launch_for(&prefix.join("codex.cmd"));
        assert_eq!(launch, Launch::direct(&native));
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn a_cmd_shim_with_a_missing_script_launches_as_it_is() {
        let prefix = temp_directory("npm-shim-missing");
        write_file(&prefix.join("codex.cmd"), NPM_SHIM);
        let shim = prefix.join("codex.cmd");
        assert_eq!(launch_for(&shim), Launch::direct(&shim));
        fs::remove_dir_all(prefix).unwrap();
    }

    #[test]
    fn a_plain_binary_launches_as_it_is() {
        let binary = Path::new("/opt/homebrew/bin/codex");
        assert_eq!(launch_for(binary), Launch::direct(binary));
        assert_eq!(command_for(binary).get_program(), binary.as_os_str());
    }
}
