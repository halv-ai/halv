//! Codex part of the account wallet.
//!
//! Each managed account is a separate `CODEX_HOME`. The shared-resource rules
//! and the daemon socket guard adapt Orca's managed Codex homes
//! (stablyai/orca, MIT).

use super::{
    files::{self, LinkOutcome},
    AccountIdentity,
};
use base64::Engine;
use serde_json::Value;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// Entries that hold the user's Codex extensions. Each account links them.
const RESOURCE_DIRS: &[&str] = &[
    "skills",
    "plugins",
    "plugin-state",
    "prompts",
    "rules",
    "memories",
    "themes",
    "hooks",
    "vendor_imports",
];
const RESOURCE_FILES: &[&str] = &["AGENTS.md"];

/// Folders that hold session rollouts. Each account links them into the
/// system default home, so Halv finds every rollout in one place and
/// `codex resume` works across accounts.
const HISTORY_DIRS: &[&str] = &["sessions", "archived_sessions"];

const OPENAI_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const OPENAI_PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const DAEMON_SOCKET: [&str; 2] = ["app-server-control", "app-server-control.sock"];

fn user_home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

/// The user's own Codex home: `CODEX_HOME`, else `~/.codex`.
pub(super) fn system_home() -> Option<PathBuf> {
    env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| user_home().map(|home| home.join(".codex")))
}

/// Reads the identity in `<home>/auth.json`. `None` means no ChatGPT login.
pub(super) fn read_identity(home: &Path) -> Option<AccountIdentity> {
    let contents = fs::read_to_string(home.join("auth.json")).ok()?;
    identity_from_auth(&contents)
}

pub(super) fn identity_from_auth(contents: &str) -> Option<AccountIdentity> {
    let auth = serde_json::from_str::<Value>(contents).ok()?;
    if auth
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
    {
        // An API-key login has no ChatGPT identity, even with stale tokens.
        return None;
    }
    let tokens = auth.get("tokens")?;
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let payload = text(tokens.get("id_token")).and_then(|token| jwt_payload(&token));
    let auth_claims = payload
        .as_ref()
        .and_then(|value| value.get(OPENAI_AUTH_CLAIM));
    let profile_claims = payload
        .as_ref()
        .and_then(|value| value.get(OPENAI_PROFILE_CLAIM));
    let email = text(payload.as_ref().and_then(|value| value.get("email")))
        .or_else(|| text(profile_claims.and_then(|claims| claims.get("email"))))?;
    let account_id = text(tokens.get("account_id"))
        .or_else(|| text(auth_claims.and_then(|claims| claims.get("chatgpt_account_id"))));
    let workspace = text(auth_claims.and_then(|claims| claims.get("workspace_name")));
    let plan = text(auth_claims.and_then(|claims| claims.get("chatgpt_plan_type")));
    Some(AccountIdentity {
        email,
        organization_id: account_id,
        organization_name: workspace,
        plan,
    })
}

fn jwt_payload(token: &str) -> Option<Value> {
    let segment = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Prepares or refreshes an account home. The step is idempotent and safe to
/// run before every launch.
pub(super) fn provision(home: &Path, system: Option<&Path>) -> Result<(), String> {
    files::create_private_dir(home)
        .map_err(|error| format!("Halv could not create the Codex account folder: {error}"))?;
    if let Some(system) = system {
        for name in HISTORY_DIRS {
            let shared = system.join(name);
            if let Err(error) = fs::create_dir_all(&shared) {
                return Err(format!(
                    "Halv could not prepare the shared Codex history folder `{name}`: {error}"
                ));
            }
            match files::link_directory(&shared, &home.join(name)) {
                Ok(LinkOutcome::Kept) => {
                    eprintln!("wallet: kept the account's own Codex `{name}` folder");
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(format!(
                        "Halv could not share the Codex history folder `{name}`: {error}"
                    ));
                }
            }
        }
        for name in RESOURCE_DIRS {
            if let Err(error) = files::link_directory(&system.join(name), &home.join(name)) {
                eprintln!("wallet: could not share the Codex `{name}` folder: {error}");
            }
        }
        for name in RESOURCE_FILES {
            if let Err(error) = files::link_file(&system.join(name), &home.join(name)) {
                eprintln!("wallet: could not share the Codex `{name}` file: {error}");
            }
        }
    }
    sync_config(system, home)?;
    if let Err(error) = files::restrict_file(&home.join("auth.json")) {
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("wallet: could not protect the Codex credential file: {error}");
        }
    }
    Ok(())
}

/// Tables that Codex writes into its own config while it runs: project
/// trust and dismissed notices. The account keeps its own entries.
const ACCOUNT_OWNED_TABLES: &[&str] = &["projects", "notice"];

/// Mirrors the user's `config.toml` into the account home.
///
/// The user's file is the source of truth. The account keeps only its own
/// entries in the tables that Codex writes. Halv forces file credentials, so
/// it can read the account identity and usage, and it turns off the Codex
/// daemon when its socket path is too long for the OS.
fn sync_config(system: Option<&Path>, home: &Path) -> Result<(), String> {
    let target = home.join("config.toml");
    let current_text = fs::read_to_string(&target).ok();
    let current = current_text
        .as_deref()
        .and_then(|text| text.parse::<toml::Table>().ok());
    let source = system
        .map(|system| system.join("config.toml"))
        .and_then(|path| fs::read_to_string(path).ok());
    let mut config = match source.as_deref() {
        Some(text) => match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(_) => {
                eprintln!("wallet: the Codex config.toml is not valid TOML; Halv did not copy it");
                current.clone().unwrap_or_default()
            }
        },
        None => toml::Table::new(),
    };
    for name in ACCOUNT_OWNED_TABLES {
        let Some(owned) = current
            .as_ref()
            .and_then(|table| table.get(*name))
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        let merged = config
            .entry((*name).to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(merged) = merged.as_table_mut() {
            for (key, entry) in owned {
                merged.insert(key.clone(), entry.clone());
            }
        }
    }
    config.insert(
        "cli_auth_credentials_store".to_string(),
        toml::Value::String("file".to_string()),
    );
    if daemon_socket_too_long(home) {
        let features = config
            .entry("features".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        match features.as_table_mut() {
            Some(features) => {
                features.insert("daemon_auto_start".to_string(), toml::Value::Boolean(false));
            }
            None => eprintln!(
                "wallet: the Codex `features` value is not a table; Codex can fail with a long socket path"
            ),
        }
    }
    let text = toml::to_string_pretty(&config)
        .map_err(|error| format!("Halv could not encode the Codex account config: {error}"))?;
    if current_text.as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    files::write_private_file(&target, text.as_bytes())
        .map_err(|error| format!("Halv could not write the Codex account config: {error}"))
}

/// Codex 0.157 and later connect to
/// `<CODEX_HOME>/app-server-control/app-server-control.sock`. The OS socket
/// path limit is 104 bytes on macOS and 108 bytes on Linux and Windows,
/// including the terminating NUL.
fn daemon_socket_too_long(home: &Path) -> bool {
    let canonical = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut socket = canonical;
    for segment in DAEMON_SOCKET {
        socket.push(segment);
    }
    let limit = if cfg!(target_os = "macos") { 103 } else { 107 };
    let length = socket.to_string_lossy().trim_start_matches(r"\\?\").len();
    length > limit
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes())
        )
    }

    #[test]
    fn identity_reads_the_id_token_claims() {
        let auth = json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt(json!({
                    "email": "dev@example.com",
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "acct-1",
                        "chatgpt_plan_type": "pro"
                    }
                })),
                "access_token": "access",
                "refresh_token": "refresh"
            }
        });
        let identity = identity_from_auth(&auth.to_string()).expect("identity");
        assert_eq!(identity.email, "dev@example.com");
        assert_eq!(identity.organization_id.as_deref(), Some("acct-1"));
        assert_eq!(identity.plan.as_deref(), Some("pro"));
    }

    #[test]
    fn identity_rejects_api_keys_and_bad_files() {
        assert_eq!(identity_from_auth(r#"{"OPENAI_API_KEY":"sk-test"}"#), None);
        assert_eq!(identity_from_auth("{not json"), None);
        assert_eq!(identity_from_auth(r#"{"tokens":{"id_token":"bad"}}"#), None);
    }

    #[test]
    fn config_sync_keeps_account_trust_and_forces_file_credentials() {
        let root =
            std::env::temp_dir().join(format!("halv-wallet-codex-{:016x}", rand::random::<u64>()));
        let system = root.join("system");
        let home = root.join("home");
        fs::create_dir_all(&system).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            system.join("config.toml"),
            "model = \"gpt-5.5\"\ncli_auth_credentials_store = \"keyring\"\n",
        )
        .unwrap();
        fs::write(
            home.join("config.toml"),
            "model = \"old\"\n[notice]\nhide_full_access_warning = true\n[projects.\"/work\"]\ntrust_level = \"trusted\"\n",
        )
        .unwrap();

        sync_config(Some(&system), &home).unwrap();

        let config = fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse::<toml::Table>()
            .unwrap();
        assert_eq!(config["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(config["cli_auth_credentials_store"].as_str(), Some("file"));
        assert_eq!(
            config["projects"]["/work"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(
            config["notice"]["hide_full_access_warning"].as_bool(),
            Some(true)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn daemon_guard_applies_only_to_long_homes() {
        assert!(!daemon_socket_too_long(Path::new("/tmp/h")));
        let long = PathBuf::from("/tmp").join("a".repeat(100));
        assert!(daemon_socket_too_long(&long));
    }
}
