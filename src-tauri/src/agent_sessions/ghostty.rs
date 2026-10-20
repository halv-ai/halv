use super::LaunchSpec;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GhosttyProbe {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub applescript_ready: bool,
}

#[cfg(target_os = "macos")]
pub(super) fn probe() -> GhosttyProbe {
    let Some(app_path) = find_app() else {
        return GhosttyProbe {
            found: false,
            version: None,
            applescript_ready: false,
        };
    };
    let version = read_version(&app_path);
    let supported = version.as_deref().is_some_and(version_is_supported);
    GhosttyProbe {
        found: true,
        version,
        applescript_ready: supported && capability_check(),
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn probe() -> GhosttyProbe {
    GhosttyProbe {
        found: false,
        version: None,
        applescript_ready: false,
    }
}

#[cfg(target_os = "macos")]
pub(super) fn launch(spec: &LaunchSpec, cwd: &Path) -> Result<(), String> {
    let probe = probe();
    if !probe.found {
        return Err("Ghostty is not installed".to_string());
    }
    if !probe.applescript_ready {
        return Err("Ghostty 1.3 or newer with AppleScript access is required".to_string());
    }
    let script = build_launch_script(spec, cwd)?;
    run_script_on_stdin(&script)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn launch(_spec: &LaunchSpec, _cwd: &Path) -> Result<(), String> {
    Err("Ghostty windows are supported only on macOS".to_string())
}

#[cfg(target_os = "macos")]
pub(super) fn is_running() -> bool {
    ["Ghostty", "ghostty"].iter().any(|name| {
        std::process::Command::new("pgrep")
            .args(["-x", name])
            .status()
            .is_ok_and(|status| status.success())
    })
}

#[cfg(not(target_os = "macos"))]
pub(super) fn is_running() -> bool {
    false
}

#[cfg(any(target_os = "macos", test))]
fn build_launch_script(spec: &LaunchSpec, cwd: &Path) -> Result<String, String> {
    let cwd = cwd
        .to_str()
        .ok_or_else(|| "the Ghostty working directory must be valid UTF-8".to_string())?;
    let command = shell_command(spec)?;
    let mut environment = Vec::with_capacity(spec.env.len() + spec.env_remove.len());
    for (name, value) in &spec.env {
        let name = name
            .to_str()
            .ok_or_else(|| "a Ghostty environment name is not valid UTF-8".to_string())?;
        let value = value
            .to_str()
            .ok_or_else(|| format!("Ghostty environment variable `{name}` is not valid UTF-8"))?;
        environment.push(format!("{name}={value}"));
    }
    for name in &spec.env_remove {
        let name = name
            .to_str()
            .ok_or_else(|| "a Ghostty environment name is not valid UTF-8".to_string())?;
        environment.push(format!("{name}="));
    }
    let environment = environment
        .iter()
        .map(|entry| format!("\"{}\"", applescript_string(entry)))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "tell application \"Ghostty\"\n\
         set surfaceConfig to new surface configuration\n\
         set initial working directory of surfaceConfig to \"{}\"\n\
         set command of surfaceConfig to \"{}\"\n\
         set environment variables of surfaceConfig to {{{environment}}}\n\
         new window with configuration surfaceConfig\n\
         end tell\n",
        applescript_string(cwd),
        applescript_string(&command),
    ))
}

#[cfg(any(target_os = "macos", test))]
fn shell_command(spec: &LaunchSpec) -> Result<String, String> {
    let program = spec
        .program
        .to_str()
        .ok_or_else(|| "the Ghostty program path must be valid UTF-8".to_string())?;
    let mut words = vec![shell_quote(program)];
    for argument in &spec.args {
        let argument = argument
            .to_str()
            .ok_or_else(|| "a Ghostty command argument is not valid UTF-8".to_string())?;
        words.push(shell_quote(argument));
    }
    Ok(words.join(" "))
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", test))]
fn applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

#[cfg(any(target_os = "macos", test))]
fn version_is_supported(version: &str) -> bool {
    let mut numbers = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok());
    let major = numbers.next().unwrap_or(0);
    let minor = numbers.next().unwrap_or(0);
    major > 1 || (major == 1 && minor >= 3)
}

#[cfg(target_os = "macos")]
const STANDARD_APP: &str = "/Applications/Ghostty.app";

/// Identifies the standard Ghostty install for the probe cache. A Ghostty
/// update rewrites its Info.plist. `None` means that Halv must search for
/// Ghostty with Spotlight.
#[cfg(target_os = "macos")]
pub(super) fn install_fingerprint() -> Option<super::probe_cache::FileFingerprint> {
    let info_plist = Path::new(STANDARD_APP).join("Contents/Info.plist");
    info_plist
        .is_file()
        .then(|| super::probe_cache::file_fingerprint(&info_plist))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn install_fingerprint() -> Option<super::probe_cache::FileFingerprint> {
    None
}

#[cfg(target_os = "macos")]
fn find_app() -> Option<std::path::PathBuf> {
    let standard = std::path::PathBuf::from(STANDARD_APP);
    if standard.is_dir() {
        return Some(standard);
    }
    let output = std::process::Command::new("mdfind")
        .arg("kMDItemCFBundleIdentifier == 'com.mitchellh.ghostty'")
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| line.ends_with(".app"))
        .map(std::path::PathBuf::from)
        .find(|path| path.is_dir())
}

#[cfg(target_os = "macos")]
fn read_version(app_path: &Path) -> Option<String> {
    let info_plist = app_path.join("Contents/Info.plist");
    let output = std::process::Command::new("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(info_plist)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn capability_check() -> bool {
    let script = "tell application \"Ghostty\"\n\
                  set surfaceConfig to new surface configuration\n\
                  set initial working directory of surfaceConfig to \"/\"\n\
                  set command of surfaceConfig to \":\"\n\
                  set environment variables of surfaceConfig to {\"HALV_GHOSTTY_PROBE=1\"}\n\
                  return version\n\
                  end tell";
    std::process::Command::new("osascript")
        .args(["-e", script])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn run_script_on_stdin(script: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = std::process::Command::new("osascript")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start Ghostty AppleScript: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Ghostty AppleScript input was unavailable".to_string())?;
    stdin
        .write_all(script.as_bytes())
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("failed to send Ghostty AppleScript: {error}"))?;
    drop(stdin);
    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for Ghostty AppleScript: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Ghostty AppleScript failed with status {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn sample_spec() -> LaunchSpec {
        LaunchSpec {
            program: PathBuf::from("/Applications/O'Reilly/agent"),
            args: vec![OsString::from("say \"hello\""), OsString::from("a'b")],
            env: vec![(
                OsString::from("API_KEY"),
                OsString::from("slash\\quote\"value"),
            )],
            env_remove: vec![OsString::from("OLD_KEY")],
        }
    }

    #[test]
    fn quotes_shell_words() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn escapes_applescript_strings() {
        assert_eq!(applescript_string("a\\b\"c\nd"), "a\\\\b\\\"c\\nd");
    }

    #[test]
    fn builds_surface_configuration_without_plain_shell_words() {
        let script = build_launch_script(&sample_spec(), Path::new("/tmp/a\"b"))
            .expect("AppleScript should build");
        assert!(script.contains("new surface configuration"));
        assert!(script.contains("initial working directory"));
        assert!(script.contains("environment variables"));
        assert!(script.contains("API_KEY=slash\\\\quote\\\"value"));
        assert!(script.contains("OLD_KEY="));
        assert!(script.contains("'a'\\\\''b'"));
    }

    #[test]
    fn requires_ghostty_1_3_or_newer() {
        assert!(!version_is_supported("1.2.9"));
        assert!(version_is_supported("1.3.0"));
        assert!(version_is_supported("2.0.0-beta"));
    }
}
