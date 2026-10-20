//! The relay URL policy of the gateway (docs/remote-protocol.md, sections 5.1 and 9).
//!
//! - Release builds use `wss://relay.halv.ai`. Debug builds use a local relay.
//! - The `HALV_RELAY_URL` variable replaces the default of both builds.
//! - The relay URL preference replaces the default. Release builds read `remoteRelayUrl`,
//!   and debug builds read `remoteRelayUrlDev`, so a local relay URL never reaches a release build.
//! - `wss://` is required. `ws://` works only for `127.0.0.1`, `::1`, and `localhost`.

use halv_remote::pairing::is_valid_relay_url;

/// The default relay URL of release builds.
pub(crate) const RELEASE_RELAY_URL: &str = "wss://relay.halv.ai";
/// The default relay URL of debug builds: a relay that runs with `wrangler dev`.
pub(crate) const DEBUG_RELAY_URL: &str = "ws://127.0.0.1:8791";
/// The variable that replaces the default relay URL.
pub(crate) const RELAY_URL_ENV: &str = "HALV_RELAY_URL";

const INVALID_RELAY_URL: &str =
    "Use a wss:// relay URL. ws:// works only for 127.0.0.1, ::1, and localhost.";

/// Returns the default relay URL of this build.
pub(crate) fn build_default_relay_url() -> &'static str {
    if cfg!(debug_assertions) {
        DEBUG_RELAY_URL
    } else {
        RELEASE_RELAY_URL
    }
}

/// Returns the default relay URL: the value of `HALV_RELAY_URL`, or the build default.
///
/// An empty or invalid variable is ignored.
pub(crate) fn default_relay_url(env_value: Option<&str>) -> String {
    let env_value = env_value.map(str::trim).filter(|value| !value.is_empty());
    match env_value.map(normalize_relay_url) {
        Some(Ok(url)) => url,
        Some(Err(_)) => {
            eprintln!(
                "remote gateway: {RELAY_URL_ENV} is not a valid relay URL; using the default"
            );
            build_default_relay_url().to_string()
        }
        None => build_default_relay_url().to_string(),
    }
}

/// Checks a relay URL from the user and removes surrounding spaces and a trailing slash.
pub(crate) fn normalize_relay_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let url = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if is_valid_relay_url(url) {
        Ok(url.to_string())
    } else {
        Err(INVALID_RELAY_URL.to_string())
    }
}

/// Returns the relay URL to use and true when it is the default.
///
/// A preference that is not valid is ignored, because the setter checks each value.
pub(crate) fn effective_relay_url(preference: Option<&str>, default: &str) -> (String, bool) {
    match preference.map(normalize_relay_url) {
        Some(Ok(url)) => (url, false),
        Some(Err(_)) => {
            eprintln!("remote gateway: the saved relay URL is not valid; using the default");
            (default.to_string(), true)
        }
        None => (default.to_string(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_default_follows_the_build_profile() {
        let expected = if cfg!(debug_assertions) {
            "ws://127.0.0.1:8791"
        } else {
            "wss://relay.halv.ai"
        };
        assert_eq!(build_default_relay_url(), expected);
        assert_eq!(default_relay_url(None), expected);
        assert_eq!(default_relay_url(Some("   ")), expected);
    }

    #[test]
    fn the_environment_variable_replaces_the_default_when_valid() {
        assert_eq!(
            default_relay_url(Some("wss://relay.example.test/")),
            "wss://relay.example.test"
        );
        assert_eq!(
            default_relay_url(Some("ws://localhost:8787")),
            "ws://localhost:8787"
        );
        assert_eq!(
            default_relay_url(Some("ws://relay.example.test")),
            build_default_relay_url()
        );
        assert_eq!(
            default_relay_url(Some("https://relay.example.test")),
            build_default_relay_url()
        );
    }

    #[test]
    fn user_urls_need_wss_except_for_loopback_hosts() {
        assert_eq!(
            normalize_relay_url(" wss://relay.halv.ai/ ").unwrap(),
            "wss://relay.halv.ai"
        );
        assert_eq!(
            normalize_relay_url("wss://relay.halv.ai:8443/base").unwrap(),
            "wss://relay.halv.ai:8443/base"
        );
        for loopback in [
            "ws://127.0.0.1:8791",
            "ws://localhost:8791",
            "ws://[::1]:8791",
        ] {
            assert_eq!(normalize_relay_url(loopback).unwrap(), loopback);
        }
        for rejected in [
            "ws://relay.halv.ai",
            "ws://192.168.1.10:8791",
            "http://relay.halv.ai",
            "https://relay.halv.ai",
            "relay.halv.ai",
            "wss://user@relay.halv.ai",
            "wss://relay.halv.ai?x=1",
            "wss://relay.halv.ai//",
            "",
        ] {
            assert!(normalize_relay_url(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn the_preference_wins_over_the_default() {
        assert_eq!(
            effective_relay_url(None, "wss://relay.halv.ai"),
            ("wss://relay.halv.ai".to_string(), true)
        );
        assert_eq!(
            effective_relay_url(Some("wss://own.example.test"), "wss://relay.halv.ai"),
            ("wss://own.example.test".to_string(), false)
        );
        assert_eq!(
            effective_relay_url(Some("ws://own.example.test"), "wss://relay.halv.ai"),
            ("wss://relay.halv.ai".to_string(), true)
        );
    }
}
