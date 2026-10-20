//! Relay authentication: the host signature (section 6.2) and the device bearer token (section 6.3).

use ed25519_dalek::{Signature, VerifyingKey};
use zeroize::Zeroizing;

use crate::b64u;
use crate::ids::HostId;
use crate::keys::{HostIdentity, TokenHash};
use crate::relay_wire::UpgradeError;

/// The header with `b64u(host_relay_public)`.
pub const HEADER_HOST_KEY: &str = "X-Halv-Host-Key";

/// The header with the time of the request in epoch milliseconds, as decimal text.
pub const HEADER_TIMESTAMP: &str = "X-Halv-Timestamp";

/// The header with the `b64u` Ed25519 signature.
pub const HEADER_SIGNATURE: &str = "X-Halv-Signature";

/// The informational client header, for example `halv-desktop/0.8.0`.
pub const HEADER_CLIENT: &str = "X-Halv-Client";

/// The header with the bearer token of a device (section 6.3).
pub const HEADER_AUTHORIZATION: &str = "Authorization";

/// The domain prefix of the signed message.
pub const HOST_AUTH_DOMAIN: &str = "halv-relay/host-auth/v1";

/// The largest allowed difference between the timestamp and the relay clock.
pub const MAX_CLOCK_SKEW_MS: u64 = 300_000;

/// Returns the signed message of section 6.2:
///
/// `"halv-relay/host-auth/v1" || "\n" || host_id || "\n" || timestamp`
pub fn host_auth_message(host_id: &HostId, timestamp_ms: u64) -> String {
    format!("{HOST_AUTH_DOMAIN}\n{host_id}\n{timestamp_ms}")
}

/// The authentication header values of a host upgrade request.
///
/// The values are not secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAuthHeaders {
    /// The value of `X-Halv-Host-Key`.
    pub host_key: String,
    /// The value of `X-Halv-Timestamp`.
    pub timestamp: String,
    /// The value of `X-Halv-Signature`.
    pub signature: String,
}

impl HostAuthHeaders {
    /// Signs the host authentication message for `timestamp_ms`.
    ///
    /// Use the current time. The relay rejects a timestamp that is not greater than the last one.
    pub fn sign(identity: &HostIdentity, timestamp_ms: u64) -> HostAuthHeaders {
        let message = host_auth_message(&identity.host_id(), timestamp_ms);
        HostAuthHeaders {
            host_key: b64u::encode(&identity.relay_public()),
            timestamp: timestamp_ms.to_string(),
            signature: b64u::encode(&identity.sign_relay(message.as_bytes())),
        }
    }

    /// Returns the three headers as (name, value) pairs.
    pub fn pairs(&self) -> [(&'static str, &str); 3] {
        [
            (HEADER_HOST_KEY, &self.host_key),
            (HEADER_TIMESTAMP, &self.timestamp),
            (HEADER_SIGNATURE, &self.signature),
        ]
    }

    /// Returns the header values as the relay receives them.
    pub fn as_request(&self) -> HostAuthRequest<'_> {
        HostAuthRequest {
            host_key: Some(&self.host_key),
            timestamp: Some(&self.timestamp),
            signature: Some(&self.signature),
        }
    }
}

/// The header values of a host upgrade request, as the relay receives them.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostAuthRequest<'a> {
    /// The value of `X-Halv-Host-Key`, if present.
    pub host_key: Option<&'a str>,
    /// The value of `X-Halv-Timestamp`, if present.
    pub timestamp: Option<&'a str>,
    /// The value of `X-Halv-Signature`, if present.
    pub signature: Option<&'a str>,
}

/// The stored state of a host object (section 6.9).
#[derive(Debug, Clone, Copy, Default)]
pub struct HostAuthState<'a> {
    /// The stored host key (`b64u`), if the host connected before.
    pub stored_host_key: Option<&'a str>,
    /// The stored `last_auth_ts`, if the host connected before.
    pub last_auth_ts: Option<u64>,
}

/// A host request that passed every check.
/// The host object stores both values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedHost {
    /// The host key (`b64u`).
    pub host_key: String,
    /// The new `last_auth_ts`.
    pub timestamp_ms: u64,
}

/// The check of section 6.2 that a host request fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostAuthError {
    /// Check 1: a header is missing or malformed (`400`).
    #[error("a host authentication header is missing or malformed")]
    Malformed,
    /// Check 2: the host key does not give the host id of the path (`401`).
    #[error("the host key does not match the host id")]
    HostIdMismatch,
    /// Check 3: the host key is not the stored host key (`401`).
    #[error("the host key does not match the stored host key")]
    HostKeyMismatch,
    /// Check 4: the timestamp is more than 300,000 ms from the relay clock (`401`, `clock_skew`).
    #[error("the timestamp is outside the allowed clock skew")]
    ClockSkew,
    /// Check 5: the timestamp is not greater than `last_auth_ts` (`401`, `replay`).
    #[error("the timestamp is not newer than the last authentication")]
    Replay,
    /// Check 6: the signature is not valid (`401`).
    #[error("the signature is not valid")]
    BadSignature,
}

impl HostAuthError {
    /// Returns the HTTP error of the upgrade response.
    pub fn upgrade_error(self) -> UpgradeError {
        match self {
            HostAuthError::Malformed => UpgradeError::BadRequest,
            HostAuthError::HostIdMismatch
            | HostAuthError::HostKeyMismatch
            | HostAuthError::BadSignature => UpgradeError::Unauthorized,
            HostAuthError::ClockSkew => UpgradeError::ClockSkew,
            HostAuthError::Replay => UpgradeError::Replay,
        }
    }
}

/// Parses canonical decimal text of a `u64`: digits only, no leading zero.
fn parse_timestamp(text: &str) -> Option<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if canonical {
        text.parse().ok()
    } else {
        None
    }
}

/// Does the host checks of section 6.2 in their order.
///
/// The relay does them for `GET /v1/hosts/{hostId}/host`.
/// The Rust relay mock and the tests use this function.
/// The timestamp must be canonical decimal text, so the signed text is unambiguous.
pub fn verify_host_auth(
    host_id: &HostId,
    request: &HostAuthRequest<'_>,
    state: &HostAuthState<'_>,
    now_ms: u64,
) -> Result<VerifiedHost, HostAuthError> {
    let host_key_text = request.host_key.ok_or(HostAuthError::Malformed)?;
    let host_key = b64u::decode_array::<32>(host_key_text).ok_or(HostAuthError::Malformed)?;
    let timestamp_ms = request
        .timestamp
        .and_then(parse_timestamp)
        .ok_or(HostAuthError::Malformed)?;
    let signature = request
        .signature
        .and_then(b64u::decode_array::<64>)
        .ok_or(HostAuthError::Malformed)?;

    if HostId::from_relay_public(&host_key) != *host_id {
        return Err(HostAuthError::HostIdMismatch);
    }
    if state
        .stored_host_key
        .is_some_and(|stored| stored != host_key_text)
    {
        return Err(HostAuthError::HostKeyMismatch);
    }
    if timestamp_ms.abs_diff(now_ms) > MAX_CLOCK_SKEW_MS {
        return Err(HostAuthError::ClockSkew);
    }
    if state.last_auth_ts.is_some_and(|last| timestamp_ms <= last) {
        return Err(HostAuthError::Replay);
    }
    let message = host_auth_message(host_id, timestamp_ms);
    let verified = VerifyingKey::from_bytes(&host_key).is_ok_and(|key| {
        key.verify_strict(message.as_bytes(), &Signature::from_bytes(&signature))
            .is_ok()
    });
    if !verified {
        return Err(HostAuthError::BadSignature);
    }
    Ok(VerifiedHost {
        host_key: host_key_text.to_owned(),
        timestamp_ms,
    })
}

/// Returns the `Authorization` header value `Bearer <b64u token>` for a relay token or an invite token.
///
/// The value holds the token. Never log it.
pub fn bearer_header_value(token: &[u8; 32]) -> Zeroizing<String> {
    let encoded = b64u::encode_secret(token);
    let mut value = Zeroizing::new(String::with_capacity(7 + encoded.len()));
    value.push_str("Bearer ");
    value.push_str(&encoded);
    value
}

/// Reads an `Authorization` header value and returns `b64u(SHA-256(token))`.
///
/// The relay does this at device admission (section 6.3).
/// It returns `None` for a missing or malformed token. The relay then answers `401`.
pub fn bearer_token_hash(authorization: &str) -> Option<TokenHash> {
    let scheme = authorization.get(..7)?;
    if !scheme.eq_ignore_ascii_case("bearer ") {
        return None;
    }
    let token = b64u::decode_array::<32>(&authorization[7..])?;
    Some(TokenHash::of(&token[..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_760_460_000_000;

    #[test]
    fn signed_message_matches_vector_v4() {
        let public: [u8; 32] = std::array::from_fn(|index| index as u8);
        let host_id = HostId::from_relay_public(&public);
        assert_eq!(
            host_auth_message(&host_id, 1_760_460_000_000),
            "halv-relay/host-auth/v1\nnYqEorAQnfAioCLxARz8mA\n1760460000000"
        );
    }

    #[test]
    fn signed_headers_verify() {
        let identity = HostIdentity::from_secrets([1u8; 32], [2u8; 32], 0);
        let headers = HostAuthHeaders::sign(&identity, NOW);
        assert_eq!(headers.timestamp, "1760460000000");
        assert_eq!(headers.host_key.len(), 43);
        assert_eq!(headers.signature.len(), 86);
        assert_eq!(
            headers.pairs()[0],
            (HEADER_HOST_KEY, headers.host_key.as_str())
        );

        let verified = verify_host_auth(
            &identity.host_id(),
            &headers.as_request(),
            &HostAuthState::default(),
            NOW + 1_000,
        )
        .unwrap();
        assert_eq!(verified.host_key, headers.host_key);
        assert_eq!(verified.timestamp_ms, NOW);

        // A second connection with a stored key and a newer timestamp passes.
        let later = HostAuthHeaders::sign(&identity, NOW + 5);
        let state = HostAuthState {
            stored_host_key: Some(&verified.host_key),
            last_auth_ts: Some(verified.timestamp_ms),
        };
        assert!(verify_host_auth(&identity.host_id(), &later.as_request(), &state, NOW).is_ok());
    }

    #[test]
    fn each_check_fails_with_its_error() {
        let identity = HostIdentity::from_secrets([1u8; 32], [2u8; 32], 0);
        let other = HostIdentity::from_secrets([3u8; 32], [4u8; 32], 0);
        let host_id = identity.host_id();
        let headers = HostAuthHeaders::sign(&identity, NOW);
        let fresh = HostAuthState::default();
        let check = |request: &HostAuthRequest<'_>, state: &HostAuthState<'_>, now: u64| {
            verify_host_auth(&host_id, request, state, now).unwrap_err()
        };

        // Check 1: missing and malformed headers.
        let mut missing = headers.as_request();
        missing.signature = None;
        assert_eq!(check(&missing, &fresh, NOW), HostAuthError::Malformed);
        let mut bad_time = headers.as_request();
        bad_time.timestamp = Some("01760460000000");
        assert_eq!(check(&bad_time, &fresh, NOW), HostAuthError::Malformed);
        let mut bad_key = headers.as_request();
        bad_key.host_key = Some("short");
        assert_eq!(check(&bad_key, &fresh, NOW), HostAuthError::Malformed);
        assert_eq!(
            HostAuthError::Malformed.upgrade_error(),
            UpgradeError::BadRequest
        );

        // Check 2: a key of another host.
        let foreign = HostAuthHeaders::sign(&other, NOW);
        assert_eq!(
            check(&foreign.as_request(), &fresh, NOW),
            HostAuthError::HostIdMismatch
        );

        // Check 3: the stored key differs.
        let stored = b64u::encode(&other.relay_public());
        let state = HostAuthState {
            stored_host_key: Some(&stored),
            last_auth_ts: None,
        };
        assert_eq!(
            check(&headers.as_request(), &state, NOW),
            HostAuthError::HostKeyMismatch
        );

        // Check 4: clock skew in both directions.
        assert_eq!(
            check(&headers.as_request(), &fresh, NOW + MAX_CLOCK_SKEW_MS + 1),
            HostAuthError::ClockSkew
        );
        assert_eq!(
            check(&headers.as_request(), &fresh, NOW - MAX_CLOCK_SKEW_MS - 1),
            HostAuthError::ClockSkew
        );
        assert!(verify_host_auth(
            &host_id,
            &headers.as_request(),
            &fresh,
            NOW + MAX_CLOCK_SKEW_MS
        )
        .is_ok());
        assert_eq!(
            HostAuthError::ClockSkew.upgrade_error().code(),
            "clock_skew"
        );

        // Check 5: a timestamp that is not newer than the last one.
        let state = HostAuthState {
            stored_host_key: None,
            last_auth_ts: Some(NOW),
        };
        assert_eq!(
            check(&headers.as_request(), &state, NOW),
            HostAuthError::Replay
        );
        assert_eq!(HostAuthError::Replay.upgrade_error().code(), "replay");

        // Check 6: a changed signature, and a signature over another timestamp.
        let mut signature = b64u::decode(&headers.signature).unwrap();
        signature[0] ^= 1;
        let changed = b64u::encode(&signature);
        let mut forged = headers.as_request();
        forged.signature = Some(&changed);
        assert_eq!(check(&forged, &fresh, NOW), HostAuthError::BadSignature);
        let mut moved = headers.as_request();
        moved.timestamp = Some("1760460000001");
        assert_eq!(check(&moved, &fresh, NOW), HostAuthError::BadSignature);
        assert_eq!(
            HostAuthError::BadSignature.upgrade_error(),
            UpgradeError::Unauthorized
        );
    }

    #[test]
    fn checks_run_in_contract_order() {
        let identity = HostIdentity::from_secrets([1u8; 32], [2u8; 32], 0);
        let headers = HostAuthHeaders::sign(&identity, NOW);
        // Skew and replay both apply: skew (check 4) comes first.
        let state = HostAuthState {
            stored_host_key: None,
            last_auth_ts: Some(NOW + 10),
        };
        assert_eq!(
            verify_host_auth(&identity.host_id(), &headers.as_request(), &state, 0).unwrap_err(),
            HostAuthError::ClockSkew
        );
    }

    #[test]
    fn timestamps_must_be_canonical() {
        assert_eq!(parse_timestamp("1760460000000"), Some(1_760_460_000_000));
        assert_eq!(parse_timestamp("0"), Some(0));
        assert_eq!(parse_timestamp("01"), None);
        assert_eq!(parse_timestamp("+1"), None);
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("1 "), None);
        assert_eq!(parse_timestamp("18446744073709551616"), None);
    }

    #[test]
    fn bearer_values() {
        let token = [0x20u8; 32];
        let value = bearer_header_value(&token);
        assert!(value.starts_with("Bearer "));
        assert_eq!(bearer_token_hash(&value), Some(TokenHash::of(&token)));
        assert_eq!(
            bearer_token_hash(&value.replace("Bearer", "bearer")),
            Some(TokenHash::of(&token))
        );
        assert_eq!(bearer_token_hash("Bearer short"), None);
        assert_eq!(bearer_token_hash("Basic abc"), None);
        assert_eq!(bearer_token_hash(""), None);
        assert_eq!(bearer_token_hash("Bearer"), None);
    }
}
