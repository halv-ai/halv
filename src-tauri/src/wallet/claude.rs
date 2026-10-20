//! Claude Code part of the account wallet.
//!
//! Each managed account is a separate `CLAUDE_CONFIG_DIR`. The layout and the
//! shared-resource rules adapt Orca's Claude profiles (stablyai/orca, MIT).

use super::{
    files::{self, LinkOutcome},
    AccountIdentity,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Folders that hold the user's Claude Code extensions. Each account links them.
const RESOURCE_DIRS: &[&str] = &[
    "skills",
    "plugins",
    "agents",
    "commands",
    "output-styles",
    "rules",
    "themes",
    "workflows",
];

/// Folders that hold session history. Each account links them into the
/// system default folder, so Halv finds every transcript in one place and
/// `--resume` works across accounts.
const HISTORY_DIRS: &[&str] = &[
    "projects",
    "sessions",
    "session-env",
    "file-history",
    "shell-snapshots",
    "todos",
    "paste-cache",
    "tasks",
    "plans",
    "transcripts",
];

const RESOURCE_FILES: &[&str] = &["CLAUDE.md"];

/// Settings that select a login. An account must never inherit them.
const PRIVATE_SETTINGS_KEYS: &[&str] = &[
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "forceLoginMethod",
    "forceLoginOrgUUID",
];
const PRIVATE_SETTINGS_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];
const SHARED_STATE_KEYS: &[&str] = &["mcpServers", "theme"];
const SETTINGS_LEDGER_FILE: &str = "settings-ledger.json";
pub(super) const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Inherited variables that would replace the account login or its keychain
/// item. A managed launch removes them.
pub(super) const LAUNCH_ENV_REMOVE: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_SECURESTORAGE_CONFIG_DIR",
];

fn user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The user's own Claude Code folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(super) fn system_config_dir() -> Option<PathBuf> {
    non_empty_env("CLAUDE_CONFIG_DIR").or_else(|| user_home().map(|home| home.join(".claude")))
}

/// Claude Code's global state file for a config folder. `None` is the system
/// default without `CLAUDE_CONFIG_DIR`, whose file sits in the user home.
pub(super) fn state_file(config_dir: Option<&Path>) -> Option<PathBuf> {
    let legacy_dir = config_dir
        .map(Path::to_path_buf)
        .or_else(|| user_home().map(|home| home.join(".claude")))?;
    let legacy = legacy_dir.join(".config.json");
    if legacy.is_file() {
        return Some(legacy);
    }
    match config_dir {
        Some(directory) => Some(directory.join(".claude.json")),
        None => user_home().map(|home| home.join(".claude.json")),
    }
}

/// The state file of the system default account.
pub(super) fn system_state_file() -> Option<PathBuf> {
    state_file(non_empty_env("CLAUDE_CONFIG_DIR").as_deref())
}

/// The keychain service Claude Code uses for a config folder.
///
/// Claude Code 2.1 appends the first 8 hex digits of SHA-256 of the
/// `CLAUDE_CONFIG_DIR` text when that variable is set. Halv passes the exact
/// same text at launch, so the hash matches.
pub(super) fn keychain_service(config_dir: Option<&Path>) -> String {
    match config_dir {
        None => KEYCHAIN_SERVICE.to_string(),
        Some(directory) => {
            let digest = Sha256::digest(directory.to_string_lossy().as_bytes());
            let suffix = digest
                .iter()
                .take(4)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("{KEYCHAIN_SERVICE}-{suffix}")
        }
    }
}

/// The keychain service of the system default account. It follows the same
/// variables as Claude Code.
pub(super) fn system_keychain_service() -> String {
    if let Some(secure) = env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        return if secure.is_empty() {
            KEYCHAIN_SERVICE.to_string()
        } else {
            keychain_service(Some(Path::new(&secure)))
        };
    }
    keychain_service(non_empty_env("CLAUDE_CONFIG_DIR").as_deref())
}

/// Reads the login a state file names. `None` means that no sign-in finished.
pub(super) fn read_identity(state_file: &Path) -> Option<AccountIdentity> {
    let contents = fs::read_to_string(state_file).ok()?;
    let state = serde_json::from_str::<Value>(&contents).ok()?;
    identity_from_state(&state)
}

fn identity_from_state(state: &Value) -> Option<AccountIdentity> {
    let account = state.get("oauthAccount")?.as_object()?;
    let text = |key: &str| {
        account
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(AccountIdentity {
        email: text("emailAddress")?,
        organization_id: text("organizationUuid"),
        organization_name: text("organizationName"),
        plan: None,
    })
}

/// Reads the subscription type from the stored credential, when Halv can
/// read it without a prompt.
pub(super) fn read_plan(home: &Path) -> Option<String> {
    let contents = read_keychain(&keychain_service(Some(home)))
        .ok()
        .flatten()
        .or_else(|| fs::read_to_string(home.join(".credentials.json")).ok())?;
    plan_from_credentials(&contents)
}

fn plan_from_credentials(contents: &str) -> Option<String> {
    serde_json::from_str::<Value>(contents)
        .ok()?
        .get("claudeAiOauth")?
        .get("subscriptionType")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Reads a Claude Code keychain item. The `Err` value carries no detail on
/// purpose, so no credential text can reach a log.
#[cfg(all(target_os = "macos", not(test)))]
pub(crate) fn read_keychain(service: &str) -> Result<Option<String>, ()> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn read_keychain(_service: &str) -> Result<Option<String>, ()> {
    Ok(None)
}

/// Deletes the keychain items Claude Code keyed to an account folder.
#[cfg(all(target_os = "macos", not(test)))]
pub(super) fn delete_keychain_items(home: &Path) {
    let plain = home.to_string_lossy().trim_end_matches('/').to_string();
    for spelling in [plain.clone(), format!("{plain}/")] {
        let service = keychain_service(Some(Path::new(&spelling)));
        // Each call removes one item. Claude Code can file one item per user name.
        for _ in 0..4 {
            let deleted = std::process::Command::new("security")
                .args(["delete-generic-password", "-s", &service])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !deleted {
                break;
            }
        }
    }
}

#[cfg(any(not(target_os = "macos"), test))]
pub(super) fn delete_keychain_items(_home: &Path) {}

/// Prepares or refreshes an account folder. The step is idempotent and safe
/// to run before every launch.
pub(super) fn provision(
    home: &Path,
    account_dir: &Path,
    default_home: Option<&Path>,
) -> Result<(), String> {
    files::create_private_dir(home)
        .map_err(|error| format!("Halv could not create the Claude account folder: {error}"))?;
    let Some(default_home) = default_home else {
        return Ok(());
    };
    for name in HISTORY_DIRS {
        let shared = default_home.join(name);
        if let Err(error) = fs::create_dir_all(&shared) {
            return Err(format!(
                "Halv could not prepare the shared Claude history folder `{name}`: {error}"
            ));
        }
        match files::link_directory(&shared, &home.join(name)) {
            Ok(LinkOutcome::Kept) => {
                eprintln!("wallet: kept the account's own Claude `{name}` folder");
            }
            Ok(_) => {}
            Err(error) => {
                return Err(format!(
                    "Halv could not share the Claude history folder `{name}`: {error}"
                ));
            }
        }
    }
    for name in RESOURCE_DIRS {
        if let Err(error) = files::link_directory(&default_home.join(name), &home.join(name)) {
            eprintln!("wallet: could not share the Claude `{name}` folder: {error}");
        }
    }
    for name in RESOURCE_FILES {
        if let Err(error) = files::link_file(&default_home.join(name), &home.join(name)) {
            eprintln!("wallet: could not share the Claude `{name}` file: {error}");
        }
    }
    if let Err(error) = sync_settings(
        &default_home.join("settings.json"),
        &home.join("settings.json"),
        &account_dir.join(SETTINGS_LEDGER_FILE),
    ) {
        eprintln!("wallet: could not share the Claude settings: {error}");
    }
    Ok(())
}

/// Copies the user's settings without the login-specific keys.
///
/// A ledger keeps the hash of the last copy. When the account changed its
/// own settings since that copy, Halv leaves them alone.
fn sync_settings(source: &Path, target: &Path, ledger: &Path) -> Result<(), String> {
    let Ok(contents) = fs::read_to_string(source) else {
        return Ok(());
    };
    let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&contents) else {
        return Ok(());
    };
    let current = fs::read(target).ok();
    let recorded = fs::read_to_string(ledger)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("settingsSha256")?.as_str().map(str::to_string));
    if let Some(current) = current.as_deref() {
        if recorded.as_deref() != Some(sha256_hex(current).as_str()) {
            return Ok(());
        }
    }
    let shared = shared_settings(settings);
    let mut text =
        serde_json::to_string_pretty(&Value::Object(shared)).map_err(|error| error.to_string())?;
    text.push('\n');
    if current.as_deref() == Some(text.as_bytes()) {
        return Ok(());
    }
    files::write_private_file(target, text.as_bytes()).map_err(|error| error.to_string())?;
    let ledger_text = serde_json::json!({ "settingsSha256": sha256_hex(text.as_bytes()) });
    files::write_private_file(ledger, ledger_text.to_string().as_bytes())
        .map_err(|error| error.to_string())
}

fn shared_settings(settings: Map<String, Value>) -> Map<String, Value> {
    settings
        .into_iter()
        .filter(|(key, _)| !PRIVATE_SETTINGS_KEYS.contains(&key.as_str()))
        .map(|(key, value)| match (key.as_str(), value) {
            ("env", Value::Object(environment)) => (
                key,
                Value::Object(
                    environment
                        .into_iter()
                        .filter(|(name, _)| !PRIVATE_SETTINGS_ENV.contains(&name.as_str()))
                        .collect(),
                ),
            ),
            (_, value) => (key, value),
        })
        .collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Finishes a new account folder after its first sign-in.
///
/// The step copies the user's MCP servers and theme into the account state
/// and marks the onboarding as complete. Without the mark, the first launch
/// opens the onboarding, where a stray Enter can start a new login.
pub(super) fn finish_sign_in(home: &Path, source_state: Option<&Path>) -> Result<(), String> {
    let Some(target) = state_file(Some(home)) else {
        return Ok(());
    };
    let contents = fs::read_to_string(&target)
        .map_err(|error| format!("Halv could not read the Claude account state: {error}"))?;
    let Ok(Value::Object(mut state)) = serde_json::from_str::<Value>(&contents) else {
        return Err("The Claude account state is not valid JSON.".to_string());
    };
    let source = source_state
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let mut changed = false;
    if let Some(Value::Object(source)) = source {
        for key in SHARED_STATE_KEYS {
            if let Some(value) = source.get(*key) {
                if !state.contains_key(*key) {
                    state.insert((*key).to_string(), value.clone());
                    changed = true;
                }
            }
        }
    }
    if state.get("hasCompletedOnboarding") != Some(&Value::Bool(true)) {
        state.insert("hasCompletedOnboarding".to_string(), Value::Bool(true));
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let text =
        serde_json::to_string_pretty(&Value::Object(state)).map_err(|error| error.to_string())?;
    files::write_private_file(&target, text.as_bytes())
        .map_err(|error| format!("Halv could not update the Claude account state: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keychain_service_matches_claude_code() {
        // Claude Code: sha256(CLAUDE_CONFIG_DIR).hex.substring(0, 8)
        let digest = Sha256::digest(b"/tmp/halv-account/home");
        let expected = format!(
            "Claude Code-credentials-{}",
            &digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()[..8]
        );
        assert_eq!(
            keychain_service(Some(Path::new("/tmp/halv-account/home"))),
            expected
        );
        assert_eq!(keychain_service(None), "Claude Code-credentials");
    }

    #[test]
    fn identity_needs_a_finished_sign_in() {
        assert_eq!(identity_from_state(&json!({ "theme": "dark" })), None);
        assert_eq!(
            identity_from_state(&json!({ "oauthAccount": { "emailAddress": "  " } })),
            None
        );
        let identity = identity_from_state(&json!({
            "oauthAccount": {
                "emailAddress": "dev@example.com",
                "organizationUuid": "org-1",
                "organizationName": "Example"
            }
        }))
        .expect("identity");
        assert_eq!(identity.email, "dev@example.com");
        assert_eq!(identity.organization_id.as_deref(), Some("org-1"));
        assert_eq!(identity.organization_name.as_deref(), Some("Example"));
    }

    #[test]
    fn plan_comes_from_the_subscription_type() {
        assert_eq!(
            plan_from_credentials(
                r#"{"claudeAiOauth":{"accessToken":"x","subscriptionType":"max"}}"#
            )
            .as_deref(),
            Some("max")
        );
        assert_eq!(plan_from_credentials("not json"), None);
    }

    #[test]
    fn shared_settings_drop_login_keys() {
        let Value::Object(settings) = json!({
            "apiKeyHelper": "/bin/key",
            "forceLoginOrgUUID": "org",
            "model": "opus",
            "env": { "ANTHROPIC_API_KEY": "secret", "FOO": "bar" }
        }) else {
            unreachable!()
        };
        let shared = Value::Object(shared_settings(settings));
        assert_eq!(shared, json!({ "model": "opus", "env": { "FOO": "bar" } }));
    }

    #[test]
    fn settings_sync_stops_after_an_account_edit() {
        let root = std::env::temp_dir().join(format!(
            "halv-wallet-settings-{:016x}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.json");
        let target = root.join("target.json");
        let ledger = root.join("ledger.json");
        fs::write(&source, r#"{"model":"opus"}"#).unwrap();
        sync_settings(&source, &target, &ledger).unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("opus"));

        fs::write(&source, r#"{"model":"sonnet"}"#).unwrap();
        sync_settings(&source, &target, &ledger).unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("sonnet"));

        fs::write(&target, r#"{"model":"haiku"}"#).unwrap();
        fs::write(&source, r#"{"model":"opus"}"#).unwrap();
        sync_settings(&source, &target, &ledger).unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("haiku"));
        fs::remove_dir_all(root).unwrap();
    }
}
