//! The host name that the phone shows: the macOS computer name, else the hostname.

/// The longest host name in a pairing offer (section 5.1).
const MAX_HOST_NAME_CHARS: usize = 64;
const FALLBACK_HOST_NAME: &str = "Halv desktop";

/// Returns the display name of this computer, at most 64 characters.
pub(crate) fn computer_name() -> String {
    let name = macos_computer_name()
        .or_else(hostname)
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok());
    clean_host_name(name.as_deref())
}

/// Trims a name and limits it to 64 characters. An empty name gets a fallback.
pub(crate) fn clean_host_name(name: Option<&str>) -> String {
    let trimmed = name.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return FALLBACK_HOST_NAME.to_string();
    }
    trimmed.chars().take(MAX_HOST_NAME_CHARS).collect()
}

#[cfg(target_os = "macos")]
fn macos_computer_name() -> Option<String> {
    command_output("/usr/sbin/scutil", &["--get", "ComputerName"])
}

#[cfg(not(target_os = "macos"))]
fn macos_computer_name() -> Option<String> {
    None
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    command_output("/bin/hostname", &[])
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
}

#[cfg(unix)]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_are_trimmed_and_limited() {
        assert_eq!(clean_host_name(Some("  Studio Mac \n")), "Studio Mac");
        assert_eq!(clean_host_name(Some("   ")), "Halv desktop");
        assert_eq!(clean_host_name(None), "Halv desktop");
        let long = "é".repeat(80);
        assert_eq!(clean_host_name(Some(&long)).chars().count(), 64);
    }

    #[test]
    fn the_computer_name_is_never_empty() {
        let name = computer_name();
        assert!(!name.is_empty());
        assert!(name.chars().count() <= 64);
    }
}
