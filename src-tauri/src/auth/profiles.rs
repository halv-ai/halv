use super::{
    AppTokens, AuthBearer, AuthError, KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT, KEYCHAIN_API_KEY_ACCOUNT,
    KEYCHAIN_CHATGPT_ACCOUNT, SERVICE_NAME,
};
use crate::storage::{CredentialProfile, Storage};
use chrono::Utc;
use reqwest::StatusCode;
use serde_json::Value;
use std::fmt;
use tauri::State;

const OPENAI_MODELS_URL: &str = "https://api.openai.com/v1/models";
const ANTHROPIC_MODELS_URL: &str = "https://api.anthropic.com/v1/models";
const ZAI_MODELS_URL: &str = "https://api.z.ai/api/paas/v4/models";
const MOONSHOT_MODELS_URL: &str = "https://api.moonshot.ai/v1/models";
const GROK_MODELS_URL: &str = "https://api.x.ai/v1/models";
const ANTHROPIC_VERSION: &str = "2023-06-01";

fn supported_credential_provider(provider: &str) -> bool {
    matches!(
        provider,
        "openai" | "anthropic" | "zai" | "moonshot" | "xai"
    )
}

#[derive(Debug)]
enum ProfileError {
    Missing {
        id: String,
    },
    Storage {
        action: &'static str,
        label: String,
    },
    WrongProvider {
        name: String,
        actual: String,
        expected: String,
    },
    Unavailable {
        name: String,
        status: String,
    },
    UnsupportedKind {
        name: String,
        kind: String,
    },
    MissingSecret {
        name: String,
    },
    Keychain {
        name: String,
        action: &'static str,
    },
    InvalidOauth {
        name: String,
    },
    Refresh {
        name: String,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { id } => write!(formatter, "Credential profile \"{id}\" was not found."),
            Self::Storage { action, label } => {
                write!(formatter, "Could not {action} credential profile \"{label}\".")
            }
            Self::WrongProvider {
                name,
                actual,
                expected,
            } => write!(
                formatter,
                "Credential profile \"{name}\" uses {actual} credentials, not {expected} credentials."
            ),
            Self::Unavailable { name, status } => write!(
                formatter,
                "Credential profile \"{name}\" has status \"{status}\" and cannot be used."
            ),
            Self::UnsupportedKind { name, kind } => write!(
                formatter,
                "Credential profile \"{name}\" has unsupported credential kind \"{kind}\"."
            ),
            Self::MissingSecret { name } => {
                write!(formatter, "Credential profile \"{name}\" has no keychain secret.")
            }
            Self::Keychain { name, action } => write!(
                formatter,
                "Could not {action} the keychain secret for credential profile \"{name}\"."
            ),
            Self::InvalidOauth { name } => write!(
                formatter,
                "Credential profile \"{name}\" has invalid stored ChatGPT tokens."
            ),
            Self::Refresh { name } => write!(
                formatter,
                "Could not refresh ChatGPT tokens for credential profile \"{name}\"."
            ),
        }
    }
}

fn profile_account_override(profile: &CredentialProfile) -> Option<String> {
    let metadata = profile.metadata_json.as_deref()?;
    serde_json::from_str::<Value>(metadata)
        .ok()?
        .get("keychainAccount")?
        .as_str()
        .map(str::trim)
        .filter(|account| !account.is_empty())
        .map(str::to_string)
}

fn per_profile_account(profile: &CredentialProfile) -> String {
    format!("{}-profile-{}", profile.provider, profile.id)
}

fn profile_account(profile: &CredentialProfile) -> String {
    profile_account_override(profile).unwrap_or_else(|| per_profile_account(profile))
}

fn get_profile_secret_with<R, E>(
    profile: &CredentialProfile,
    read: R,
) -> Result<Option<String>, String>
where
    R: FnOnce(&str, &str) -> Result<Option<String>, E>,
{
    let account = profile_account(profile);
    read(SERVICE_NAME, &account).map_err(|_| {
        ProfileError::Keychain {
            name: profile.name.clone(),
            action: "read",
        }
        .to_string()
    })
}

fn set_profile_secret_with<W, E>(
    profile: &CredentialProfile,
    secret: &str,
    write: W,
) -> Result<(), String>
where
    W: FnOnce(&str, &str, &str) -> Result<(), E>,
{
    let account = profile_account(profile);
    write(SERVICE_NAME, &account, secret).map_err(|_| {
        ProfileError::Keychain {
            name: profile.name.clone(),
            action: "write",
        }
        .to_string()
    })
}

fn delete_profile_secret_with<D, E>(profile: &CredentialProfile, delete: D) -> Result<(), String>
where
    D: FnOnce(&str, &str) -> Result<(), E>,
{
    let account = profile_account(profile);
    delete(SERVICE_NAME, &account).map_err(|_| {
        ProfileError::Keychain {
            name: profile.name.clone(),
            action: "delete",
        }
        .to_string()
    })
}

fn get_profile_secret(profile: &CredentialProfile) -> Result<Option<String>, String> {
    get_profile_secret_with(profile, super::read_keychain_secret)
}

fn set_profile_secret(profile: &CredentialProfile, secret: &str) -> Result<(), String> {
    set_profile_secret_with(profile, secret, super::write_keychain_secret)
}

fn delete_profile_secret(profile: &CredentialProfile) -> Result<(), String> {
    delete_profile_secret_with(profile, super::delete_keychain_secret)
}

pub fn ensure_default_credential_profiles(storage: &Storage) {
    ensure_default_credential_profiles_with(storage, super::get_keychain_secret);
}

pub fn ensure_chatgpt_oauth_profile(storage: &Storage) -> Result<(), String> {
    ensure_chatgpt_oauth_profile_with(storage, || Utc::now().to_rfc3339())
}

fn ensure_chatgpt_oauth_profile_with<N>(storage: &Storage, now: N) -> Result<(), String>
where
    N: FnOnce() -> String,
{
    let profiles = storage.list_credential_profiles().map_err(|_| {
        ProfileError::Storage {
            action: "list",
            label: "ChatGPT".to_string(),
        }
        .to_string()
    })?;

    if let Some(profile) = profiles
        .iter()
        .find(|profile| profile_account(profile) == KEYCHAIN_CHATGPT_ACCOUNT)
    {
        if profile.status != "active" {
            storage
                .set_credential_profile_status(&profile.id, "active")
                .map_err(|_| {
                    ProfileError::Storage {
                        action: "activate",
                        label: profile.name.clone(),
                    }
                    .to_string()
                })?;
        }
        return Ok(());
    }

    let timestamp = now();
    let profile = CredentialProfile {
        id: crate::storage::new_uuid_v4(),
        provider: "openai".to_string(),
        name: "ChatGPT".to_string(),
        kind: "oauth".to_string(),
        priority: 0,
        status: "active".to_string(),
        last_validated_at: None,
        metadata_json: Some(
            serde_json::json!({ "keychainAccount": KEYCHAIN_CHATGPT_ACCOUNT }).to_string(),
        ),
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    storage.insert_credential_profile(&profile).map_err(|_| {
        ProfileError::Storage {
            action: "add",
            label: profile.name,
        }
        .to_string()
    })
}

fn ensure_default_credential_profiles_with<R, E>(storage: &Storage, mut read: R)
where
    R: FnMut(&str) -> Result<Option<String>, E>,
{
    match storage.list_credential_profiles() {
        Ok(profiles) if !profiles.is_empty() => return,
        Ok(_) => {}
        Err(_) => {
            eprintln!("Credential profile backfill could not read the profile table.");
            return;
        }
    }

    let defaults = [
        (KEYCHAIN_CHATGPT_ACCOUNT, "openai", "ChatGPT", "oauth"),
        (
            KEYCHAIN_API_KEY_ACCOUNT,
            "openai",
            "OpenAI API key",
            "api_key",
        ),
        (
            KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT,
            "anthropic",
            "Anthropic API key",
            "api_key",
        ),
    ];

    for (account, provider, name, kind) in defaults {
        match read(account) {
            Ok(Some(_)) => {}
            Ok(None) => continue,
            Err(_) => {
                eprintln!("Credential profile backfill could not read keychain account {account}.");
                continue;
            }
        }

        let now = Utc::now().to_rfc3339();
        let profile = CredentialProfile {
            id: crate::storage::new_uuid_v4(),
            provider: provider.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            priority: 0,
            status: "active".to_string(),
            last_validated_at: None,
            metadata_json: Some(serde_json::json!({ "keychainAccount": account }).to_string()),
            created_at: now.clone(),
            updated_at: now,
        };
        if storage.insert_credential_profile(&profile).is_err() {
            eprintln!("Credential profile backfill could not add profile {name}.");
        }
    }
}

fn load_profile(storage: &Storage, id: &str) -> Result<CredentialProfile, String> {
    storage
        .get_credential_profile(id)
        .map_err(|_| {
            ProfileError::Storage {
                action: "load",
                label: id.to_string(),
            }
            .to_string()
        })?
        .ok_or_else(|| ProfileError::Missing { id: id.to_string() }.to_string())
}

fn load_active_profile(
    storage: &Storage,
    id: &str,
    expected_provider: &str,
) -> Result<CredentialProfile, String> {
    let profile = load_profile(storage, id)?;
    if profile.provider != expected_provider {
        return Err(ProfileError::WrongProvider {
            name: profile.name.clone(),
            actual: profile.provider.clone(),
            expected: expected_provider.to_string(),
        }
        .to_string());
    }
    if profile.status != "active" {
        return Err(ProfileError::Unavailable {
            name: profile.name.clone(),
            status: profile.status.clone(),
        }
        .to_string());
    }
    Ok(profile)
}

pub(crate) fn active_api_key_with<R, E>(
    storage: &Storage,
    id: &str,
    provider: &str,
    read: R,
) -> Result<(CredentialProfile, String), String>
where
    R: FnOnce(&str, &str) -> Result<Option<String>, E>,
{
    let profile = load_active_profile(storage, id, provider)?;
    if profile.kind != "api_key" {
        return Err(ProfileError::UnsupportedKind {
            name: profile.name.clone(),
            kind: profile.kind.clone(),
        }
        .to_string());
    }
    let secret = get_profile_secret_with(&profile, read)?.ok_or_else(|| {
        ProfileError::MissingSecret {
            name: profile.name.clone(),
        }
        .to_string()
    })?;
    Ok((profile, secret))
}

pub(crate) fn resolve_agent_api_key_profile(
    storage: &Storage,
    profile_id: &str,
    provider: &str,
) -> Result<String, String> {
    let (_, secret) =
        active_api_key_with(storage, profile_id, provider, super::read_keychain_secret)?;
    Ok(secret)
}

pub(crate) async fn auth_get_bearer_for_profile(
    storage: &Storage,
    profile_id: Option<&str>,
) -> Result<AuthBearer, String> {
    let Some(id) = profile_id else {
        return super::auth_get_bearer().await;
    };
    let profile = load_active_profile(storage, id, "openai")?;
    match profile.kind.as_str() {
        "api_key" => {
            let secret = get_profile_secret(&profile)?.ok_or_else(|| {
                ProfileError::MissingSecret {
                    name: profile.name.clone(),
                }
                .to_string()
            })?;
            Ok(AuthBearer {
                token: secret,
                source: "api-key".to_string(),
                account_id: None,
                is_api_key: true,
            })
        }
        "oauth" => resolve_oauth_bearer(&profile).await,
        kind => Err(ProfileError::UnsupportedKind {
            name: profile.name.clone(),
            kind: kind.to_string(),
        }
        .to_string()),
    }
}

async fn resolve_oauth_bearer(profile: &CredentialProfile) -> Result<AuthBearer, String> {
    let secret = get_profile_secret(profile)?.ok_or_else(|| {
        ProfileError::MissingSecret {
            name: profile.name.clone(),
        }
        .to_string()
    })?;
    let tokens: AppTokens = serde_json::from_str(&secret).map_err(|_| {
        ProfileError::InvalidOauth {
            name: profile.name.clone(),
        }
        .to_string()
    })?;
    let account = profile_account(profile);
    let tokens = match super::ensure_app_access_token_for_account(tokens, &account).await {
        Ok(tokens) => tokens,
        Err(AuthError::PermanentRefresh {
            failed_refresh_token,
            ..
        }) => {
            let _ = super::clear_app_tokens_if_refresh_token_matches_for_account(
                &account,
                &failed_refresh_token,
            );
            return Err(ProfileError::Refresh {
                name: profile.name.clone(),
            }
            .to_string());
        }
        Err(_) => {
            return Err(ProfileError::Refresh {
                name: profile.name.clone(),
            }
            .to_string())
        }
    };
    let claims = super::parse_id_token_claims(&tokens.id_token);
    let account_id = claims.account_id.or(tokens.account_id).ok_or_else(|| {
        ProfileError::InvalidOauth {
            name: profile.name.clone(),
        }
        .to_string()
    })?;
    Ok(AuthBearer {
        token: tokens.access_token,
        source: "app".to_string(),
        account_id: Some(account_id),
        is_api_key: false,
    })
}

pub(crate) fn auth_get_anthropic_api_key_for_profile(
    storage: &Storage,
    profile_id: Option<&str>,
) -> Result<String, String> {
    let Some(id) = profile_id else {
        return super::auth_get_anthropic_api_key();
    };
    let (_, secret) = active_api_key_with(storage, id, "anthropic", super::read_keychain_secret)?;
    Ok(secret)
}

async fn validate_openai_token(profile_name: &str, secret: &str) -> Result<(), String> {
    let response = super::api_client()
        .get(OPENAI_MODELS_URL)
        .bearer_auth(secret)
        .send()
        .await
        .map_err(|_| format!("Could not validate credential profile \"{profile_name}\"."))?;
    validation_response(profile_name, "OpenAI", response.status())
}

async fn validate_anthropic_key(profile_name: &str, secret: &str) -> Result<(), String> {
    let response = super::api_client()
        .get(ANTHROPIC_MODELS_URL)
        .header("x-api-key", secret)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .send()
        .await
        .map_err(|_| format!("Could not validate credential profile \"{profile_name}\"."))?;
    validation_response(profile_name, "Anthropic", response.status())
}

async fn validate_bearer_key(
    profile_name: &str,
    provider_name: &str,
    models_url: &str,
    secret: &str,
) -> Result<(), String> {
    let response = super::api_client()
        .get(models_url)
        .bearer_auth(secret)
        .send()
        .await
        .map_err(|_| format!("Could not validate credential profile \"{profile_name}\"."))?;
    validation_response(profile_name, provider_name, response.status())
}

fn validation_response(
    profile_name: &str,
    provider_name: &str,
    status: StatusCode,
) -> Result<(), String> {
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(format!(
            "{provider_name} rejected the secret for credential profile \"{profile_name}\"."
        ));
    }
    Err(format!(
        "Could not validate credential profile \"{profile_name}\" because {provider_name} returned HTTP {status}."
    ))
}

fn validate_resolved_oauth_bearer(bearer: Result<AuthBearer, String>) -> Result<(), String> {
    bearer.map(|_| ())
}

async fn validate_profile(profile: &CredentialProfile) -> Result<(), String> {
    let secret = get_profile_secret(profile)?.ok_or_else(|| {
        ProfileError::MissingSecret {
            name: profile.name.clone(),
        }
        .to_string()
    })?;
    match (profile.provider.as_str(), profile.kind.as_str()) {
        ("openai", "api_key") => validate_openai_token(&profile.name, &secret).await,
        ("anthropic", "api_key") => validate_anthropic_key(&profile.name, &secret).await,
        ("zai", "api_key") => {
            validate_bearer_key(&profile.name, "GLM (Z.ai)", ZAI_MODELS_URL, &secret).await
        }
        ("moonshot", "api_key") => {
            validate_bearer_key(&profile.name, "Moonshot", MOONSHOT_MODELS_URL, &secret).await
        }
        ("xai", "api_key") => {
            validate_bearer_key(&profile.name, "Grok (xAI)", GROK_MODELS_URL, &secret).await
        }
        ("openai", "oauth") => validate_resolved_oauth_bearer(resolve_oauth_bearer(profile).await),
        (_, kind) => Err(ProfileError::UnsupportedKind {
            name: profile.name.clone(),
            kind: kind.to_string(),
        }
        .to_string()),
    }
}

fn update_validation_state(
    storage: &Storage,
    profile: &CredentialProfile,
    status: &str,
    validated_at: &str,
) -> Result<CredentialProfile, String> {
    let profile = storage
        .set_credential_profile_status(&profile.id, status)
        .map_err(|_| {
            ProfileError::Storage {
                action: "update",
                label: profile.name.clone(),
            }
            .to_string()
        })?;
    storage
        .set_credential_profile_validated(
            &profile.id,
            Some(validated_at),
            profile.metadata_json.as_deref(),
        )
        .map_err(|_| {
            ProfileError::Storage {
                action: "update",
                label: profile.name.clone(),
            }
            .to_string()
        })
}

#[tauri::command]
pub async fn credential_profile_add(
    storage: State<'_, Storage>,
    provider: String,
    name: String,
    kind: String,
    secret: String,
) -> Result<CredentialProfile, String> {
    let provider = provider.trim().to_ascii_lowercase();
    if !supported_credential_provider(&provider) {
        return Err(
            "Credential provider must be \"openai\", \"anthropic\", \"zai\", \"moonshot\", or \"xai\"."
                .to_string(),
        );
    }
    if kind == "oauth" {
        return Err(
            "OAuth profiles cannot be added yet. Use the existing ChatGPT login.".to_string(),
        );
    }
    if kind != "api_key" {
        return Err("Credential kind must be \"api_key\".".to_string());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Credential profile name cannot be empty.".to_string());
    }
    let secret = secret.trim();
    if secret.is_empty() {
        return Err("Credential profile secret cannot be empty.".to_string());
    }

    match provider.as_str() {
        "openai" => validate_openai_token(name, secret).await?,
        "anthropic" => validate_anthropic_key(name, secret).await?,
        "zai" => validate_bearer_key(name, "GLM (Z.ai)", ZAI_MODELS_URL, secret).await?,
        "moonshot" => validate_bearer_key(name, "Moonshot", MOONSHOT_MODELS_URL, secret).await?,
        "xai" => validate_bearer_key(name, "Grok (xAI)", GROK_MODELS_URL, secret).await?,
        _ => unreachable!(),
    }

    let now = Utc::now().to_rfc3339();
    let profile = CredentialProfile {
        id: crate::storage::new_uuid_v4(),
        provider,
        name: name.to_string(),
        kind,
        priority: 0,
        status: "active".to_string(),
        last_validated_at: Some(now.clone()),
        metadata_json: None,
        created_at: now.clone(),
        updated_at: now,
    };
    set_profile_secret(&profile, secret)?;
    if storage.insert_credential_profile(&profile).is_err() {
        if delete_profile_secret(&profile).is_err() {
            eprintln!(
                "Credential profile add could not remove orphaned keychain account {}.",
                per_profile_account(&profile)
            );
        }
        return Err(format!(
            "Could not add credential profile \"{}\".",
            profile.name
        ));
    }
    Ok(profile)
}

#[tauri::command]
pub async fn credential_profile_remove(
    storage: State<'_, Storage>,
    id: String,
) -> Result<(), String> {
    let profile = load_profile(&storage, &id)?;
    if profile_account_override(&profile).is_none() {
        delete_profile_secret(&profile)?;
    }
    storage.delete_credential_profile(&id).map_err(|_| {
        ProfileError::Storage {
            action: "remove",
            label: profile.name,
        }
        .to_string()
    })
}

#[tauri::command]
pub async fn credential_profile_rename(
    storage: State<'_, Storage>,
    id: String,
    name: String,
) -> Result<CredentialProfile, String> {
    let profile = load_profile(&storage, &id)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("Credential profile name cannot be empty.".to_string());
    }
    storage.rename_credential_profile(&id, name).map_err(|_| {
        ProfileError::Storage {
            action: "rename",
            label: profile.name,
        }
        .to_string()
    })
}

#[tauri::command]
pub async fn credential_profile_set_enabled(
    storage: State<'_, Storage>,
    id: String,
    enabled: bool,
) -> Result<CredentialProfile, String> {
    let profile = load_profile(&storage, &id)?;
    if !enabled {
        if profile.status == "invalid" || profile.status == "disabled" {
            return Ok(profile);
        }
        return storage
            .set_credential_profile_status(&id, "disabled")
            .map_err(|_| {
                ProfileError::Storage {
                    action: "disable",
                    label: profile.name,
                }
                .to_string()
            });
    }
    if profile.status == "active" {
        return Ok(profile);
    }
    if profile.status == "invalid" {
        let validated_at = Utc::now().to_rfc3339();
        if validate_profile(&profile).await.is_err() {
            let _ = update_validation_state(&storage, &profile, "invalid", &validated_at)?;
            return Err(format!(
                "Credential profile \"{}\" is still invalid.",
                profile.name
            ));
        }
        return update_validation_state(&storage, &profile, "active", &validated_at);
    }
    storage
        .set_credential_profile_status(&id, "active")
        .map_err(|_| {
            ProfileError::Storage {
                action: "enable",
                label: profile.name,
            }
            .to_string()
        })
}

#[tauri::command]
pub async fn credential_profile_set_priority(
    storage: State<'_, Storage>,
    id: String,
    priority: i64,
) -> Result<CredentialProfile, String> {
    let profile = load_profile(&storage, &id)?;
    storage
        .set_credential_profile_priority(&id, priority)
        .map_err(|_| {
            ProfileError::Storage {
                action: "set priority for",
                label: profile.name,
            }
            .to_string()
        })
}

#[tauri::command]
pub async fn credential_profile_test(
    storage: State<'_, Storage>,
    id: String,
) -> Result<CredentialProfile, String> {
    let profile = load_profile(&storage, &id)?;
    let validated_at = Utc::now().to_rfc3339();
    let status = if validate_profile(&profile).await.is_ok() {
        "active"
    } else {
        "invalid"
    };
    update_validation_state(&storage, &profile, status, &validated_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;

    fn test_storage(name: &str) -> (Storage, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "halv-auth-profiles-{name}-{}.sqlite3",
            crate::storage::new_uuid_v4()
        ));
        let storage = Storage::open(&path).expect("profile test storage should open");
        (storage, path)
    }

    fn test_profile(
        id: &str,
        provider: &str,
        name: &str,
        kind: &str,
        status: &str,
        metadata_json: Option<&str>,
    ) -> CredentialProfile {
        CredentialProfile {
            id: id.to_string(),
            provider: provider.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            priority: 0,
            status: status.to_string(),
            last_validated_at: None,
            metadata_json: metadata_json.map(str::to_string),
            created_at: "2000-01-01T00:00:00Z".to_string(),
            updated_at: "2000-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn resolved_oauth_bearer_completes_validation_without_api_request() {
        let bearer = AuthBearer {
            token: "oauth-access-token".to_string(),
            source: "app".to_string(),
            account_id: Some("account-id".to_string()),
            is_api_key: false,
        };

        assert!(validate_resolved_oauth_bearer(Ok(bearer)).is_ok());
    }

    #[test]
    fn xai_is_a_supported_credential_provider() {
        assert!(supported_credential_provider("xai"));
        assert!(!supported_credential_provider("grok"));
        assert_eq!(GROK_MODELS_URL, "https://api.x.ai/v1/models");
    }

    #[test]
    fn chatgpt_oauth_profile_is_created_when_absent() {
        let (storage, path) = test_storage("chatgpt-create");

        ensure_chatgpt_oauth_profile_with(&storage, || "2026-08-21T12:00:00Z".to_string())
            .expect("ChatGPT profile should be created");

        let profiles = storage
            .list_credential_profiles()
            .expect("ChatGPT profile should list");
        assert_eq!(profiles.len(), 1);
        let profile = &profiles[0];
        assert_eq!(profile.provider, "openai");
        assert_eq!(profile.name, "ChatGPT");
        assert_eq!(profile.kind, "oauth");
        assert_eq!(profile.priority, 0);
        assert_eq!(profile.status, "active");
        assert_eq!(profile.created_at, "2026-08-21T12:00:00Z");
        assert_eq!(profile.updated_at, "2026-08-21T12:00:00Z");
        assert_eq!(
            profile_account_override(profile).as_deref(),
            Some(KEYCHAIN_CHATGPT_ACCOUNT)
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn chatgpt_oauth_profile_is_idempotent() {
        let (storage, path) = test_storage("chatgpt-idempotent");
        let profile = test_profile(
            "chatgpt-existing",
            "openai",
            "ChatGPT",
            "oauth",
            "active",
            Some(&serde_json::json!({ "keychainAccount": KEYCHAIN_CHATGPT_ACCOUNT }).to_string()),
        );
        storage
            .insert_credential_profile(&profile)
            .expect("existing ChatGPT profile should insert");

        ensure_chatgpt_oauth_profile_with(&storage, || {
            panic!("an existing profile must not request a timestamp")
        })
        .expect("existing ChatGPT profile should remain unchanged");

        assert_eq!(
            storage
                .list_credential_profiles()
                .expect("ChatGPT profiles should list"),
            vec![profile]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn chatgpt_oauth_profile_reactivates_a_disabled_profile() {
        let (storage, path) = test_storage("chatgpt-reactivate");
        let profile = test_profile(
            "chatgpt-disabled",
            "openai",
            "ChatGPT",
            "oauth",
            "disabled",
            Some(&serde_json::json!({ "keychainAccount": KEYCHAIN_CHATGPT_ACCOUNT }).to_string()),
        );
        let previous_updated_at = profile.updated_at.clone();
        storage
            .insert_credential_profile(&profile)
            .expect("disabled ChatGPT profile should insert");

        ensure_chatgpt_oauth_profile_with(&storage, || {
            panic!("an existing profile must not request a timestamp")
        })
        .expect("disabled ChatGPT profile should reactivate");

        let profiles = storage
            .list_credential_profiles()
            .expect("ChatGPT profile should list");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, profile.id);
        assert_eq!(profiles[0].status, "active");
        assert_ne!(profiles[0].updated_at, previous_updated_at);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn backfill_adds_profiles_for_all_legacy_secrets() {
        let (storage, path) = test_storage("backfill");
        let secrets = HashMap::from([
            (KEYCHAIN_CHATGPT_ACCOUNT, "oauth-json"),
            (KEYCHAIN_API_KEY_ACCOUNT, "openai-secret"),
            (KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT, "anthropic-secret"),
        ]);

        ensure_default_credential_profiles_with(&storage, |account| {
            Ok::<_, String>(secrets.get(account).map(|secret| (*secret).to_string()))
        });

        let profiles = storage
            .list_credential_profiles()
            .expect("backfilled profiles should list");
        assert_eq!(profiles.len(), 3);
        for (name, account, provider, kind) in [
            ("ChatGPT", KEYCHAIN_CHATGPT_ACCOUNT, "openai", "oauth"),
            (
                "OpenAI API key",
                KEYCHAIN_API_KEY_ACCOUNT,
                "openai",
                "api_key",
            ),
            (
                "Anthropic API key",
                KEYCHAIN_ANTHROPIC_API_KEY_ACCOUNT,
                "anthropic",
                "api_key",
            ),
        ] {
            let profile = profiles
                .iter()
                .find(|profile| profile.name == name)
                .expect("backfilled profile should exist");
            assert_eq!(profile.provider, provider);
            assert_eq!(profile.kind, kind);
            assert_eq!(profile_account_override(profile).as_deref(), Some(account));
        }

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn backfill_does_nothing_when_a_profile_exists() {
        let (storage, path) = test_storage("backfill-existing");
        let profile = test_profile("existing", "openai", "Existing", "api_key", "active", None);
        storage
            .insert_credential_profile(&profile)
            .expect("existing profile should insert");

        ensure_default_credential_profiles_with(&storage, |_| -> Result<Option<String>, String> {
            panic!("non-empty backfill must not read the keychain")
        });

        assert_eq!(
            storage
                .list_credential_profiles()
                .expect("profiles should list"),
            vec![profile]
        );
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn backfill_ignores_unavailable_keychain() {
        let (storage, path) = test_storage("backfill-no-keychain");
        ensure_default_credential_profiles_with(&storage, |_| {
            Err::<Option<String>, _>("keychain failed with fake-secret")
        });
        assert!(storage
            .list_credential_profiles()
            .expect("profiles should list")
            .is_empty());
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pinned_profile_errors_do_not_fall_back() {
        let (storage, path) = test_storage("resolution-errors");
        let disabled = test_profile(
            "disabled",
            "openai",
            "Disabled key",
            "api_key",
            "disabled",
            None,
        );
        storage
            .insert_credential_profile(&disabled)
            .expect("disabled profile should insert");

        let disabled_error = active_api_key_with(
            &storage,
            "disabled",
            "openai",
            |_, _| -> Result<Option<String>, String> {
                panic!("disabled profile must not read or fall back")
            },
        )
        .expect_err("disabled profile should fail");
        assert!(disabled_error.contains("Disabled key"));

        let missing_error = active_api_key_with(
            &storage,
            "missing",
            "openai",
            |_, _| -> Result<Option<String>, String> {
                panic!("missing profile must not read or fall back")
            },
        )
        .expect_err("missing profile should fail");
        assert!(missing_error.contains("missing"));

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn api_key_resolution_uses_injected_profile_reader() {
        let (storage, path) = test_storage("resolution-reader");
        let profile = test_profile("profile-id", "openai", "Primary", "api_key", "active", None);
        storage
            .insert_credential_profile(&profile)
            .expect("profile should insert");

        let (_, secret) =
            active_api_key_with(&storage, &profile.id, "openai", |service, account| {
                assert_eq!(service, SERVICE_NAME);
                assert_eq!(account, "openai-profile-profile-id");
                Ok::<_, String>(Some("fake-profile-secret".to_string()))
            })
            .expect("profile should resolve");
        assert_eq!(secret, "fake-profile-secret");

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_account_profile_reads_the_legacy_account() {
        let (storage, path) = test_storage("resolution-legacy-account");
        let profile = test_profile(
            "legacy",
            "openai",
            "Legacy key",
            "api_key",
            "active",
            Some(r#"{"keychainAccount":"openai-api-key"}"#),
        );
        storage
            .insert_credential_profile(&profile)
            .expect("profile should insert");

        let (_, secret) =
            active_api_key_with(&storage, &profile.id, "openai", |service, account| {
                assert_eq!(service, SERVICE_NAME);
                assert_eq!(account, KEYCHAIN_API_KEY_ACCOUNT);
                Ok::<_, String>(Some("legacy-secret".to_string()))
            })
            .expect("legacy profile should resolve");
        assert_eq!(secret, "legacy-secret");

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn profile_keychain_errors_redact_secrets() {
        let secret = "fake-secret-value";
        let profile = test_profile(
            "redaction",
            "openai",
            "Redaction",
            "api_key",
            "active",
            None,
        );

        let read_error = get_profile_secret_with(&profile, |_, _| {
            Err::<Option<String>, _>(format!("read failed: {secret}"))
        })
        .expect_err("read should fail");
        let write_error = set_profile_secret_with(&profile, secret, |_, _, _| {
            Err::<(), _>(format!("write failed: {secret}"))
        })
        .expect_err("write should fail");
        let delete_error = delete_profile_secret_with(&profile, |_, _| {
            Err::<(), _>(format!("delete failed: {secret}"))
        })
        .expect_err("delete should fail");

        for error in [read_error, write_error, delete_error] {
            assert!(!error.contains(secret));
        }
    }

    #[test]
    fn resolver_keychain_error_redacts_secret() {
        let (storage, path) = test_storage("resolver-redaction");
        let secret = "fake-secret-value";
        let profile = test_profile(
            "redaction",
            "openai",
            "Redaction",
            "api_key",
            "active",
            None,
        );
        storage
            .insert_credential_profile(&profile)
            .expect("profile should insert");

        let error = active_api_key_with(&storage, &profile.id, "openai", |_, _| {
            Err::<Option<String>, _>(format!("reader exposed {secret}"))
        })
        .expect_err("resolver should fail");
        assert!(!error.contains(secret));

        drop(storage);
        let _ = fs::remove_file(path);
    }
}
