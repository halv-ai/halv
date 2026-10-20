//! Loads newer price tables from the Halv server (ADR 0015).
//!
//! Rules:
//! - At start, Halv reads the cached copy in the app data folder. It uses the
//!   copy only when the copy is valid and newer than the embedded table.
//! - Then Halv asks `GET /v1/prices` once per day. A valid, newer answer
//!   replaces the active table and the cached copy.
//! - Any error keeps the active table. Prices never block the app.

use super::{install, PriceTable};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::{AppHandle, Manager};

const CACHE_FILE_NAME: &str = "prices.json";
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST_REFRESH_DELAY: Duration = Duration::from_secs(60);
const MAX_PRICE_BODY_BYTES: usize = 512 * 1024;

pub(crate) fn cache_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CACHE_FILE_NAME)
}

/// Reads the cached table and installs it when it is newer.
pub(crate) fn load_cached(app_data_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(cache_path(app_data_dir)).ok()?;
    let table = PriceTable::parse(&text).ok()?;
    let version = table.price_version.clone();
    install(table).then_some(version)
}

/// Checks a server answer. Returns the table when it is valid and newer
/// than `active_version`.
pub(crate) fn accept_answer(
    body: &str,
    active_version: &str,
) -> Result<Option<PriceTable>, String> {
    if body.len() > MAX_PRICE_BODY_BYTES {
        return Err("the price table is too large".to_string());
    }
    let table = PriceTable::parse(body)?;
    Ok((table.price_version.as_str() > active_version).then_some(table))
}

pub(crate) fn spawn(app: AppHandle) {
    let Ok(app_data_dir) = crate::app_data::resolved_app_data_dir(&app) else {
        return;
    };
    if let Some(version) = load_cached(&app_data_dir) {
        eprintln!("Halv uses the cached price table {version}");
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_REFRESH_DELAY).await;
        loop {
            if let Err(error) = refresh_once(&app, &app_data_dir).await {
                eprintln!("Could not refresh the price table: {error}");
            }
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
}

async fn refresh_once(app: &AppHandle, app_data_dir: &Path) -> Result<(), String> {
    let storage = app.state::<crate::storage::Storage>();
    let server_url = crate::auth::auth_server_url(&storage).map_err(|error| error.to_string())?;
    let response = crate::auth::api_client()
        .get(format!("{}/v1/prices", server_url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("could not read the answer: {error}"))?;
    let active_version = super::current().price_version.clone();
    let Some(table) = accept_answer(&body, &active_version)? else {
        return Ok(());
    };
    let version = table.price_version.clone();
    if install(table) {
        let _ = fs::create_dir_all(app_data_dir);
        fs::write(cache_path(app_data_dir), body)
            .map_err(|error| format!("could not cache the price table: {error}"))?;
        eprintln!("Halv uses the price table {version} from the server");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_valid_newer_tables() {
        let embedded = super::super::EMBEDDED_PRICES;
        let version = super::super::embedded_price_version();
        assert!(accept_answer(embedded, version).unwrap().is_none());
        assert!(accept_answer(embedded, "2020-01-01").unwrap().is_some());
        assert!(accept_answer("{}", "2020-01-01").is_err());
        assert!(accept_answer(&"x".repeat(MAX_PRICE_BODY_BYTES + 1), "2020-01-01").is_err());
    }

    #[test]
    fn an_invalid_cache_file_is_ignored() {
        let dir =
            std::env::temp_dir().join(format!("halv-prices-{}", crate::storage::new_uuid_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(cache_path(&dir), "not json").unwrap();
        assert_eq!(load_cached(&dir), None);
        let _ = fs::remove_dir_all(dir);
    }
}
