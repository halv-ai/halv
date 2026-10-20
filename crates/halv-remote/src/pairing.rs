//! The pairing offer (section 5).
//!
//! The desktop shows the offer as a `halv://pair?p=<b64u(JSON)>` URI.
//! The URI holds the pairing secret. Never log it.

use std::fmt;

use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::b64u;
use crate::error::{Error, OfferError, Result};
use crate::ids::HostId;
use crate::keys::{is_low_order_x25519, HostIdentity, TokenHash};
use crate::secret::{random_array, secret32};

/// The pairing offer version of this crate.
pub const OFFER_VERSION: u64 = 1;

/// The text before the `b64u` payload of an offer URI.
pub const OFFER_URI_PREFIX: &str = "halv://pair?p=";

/// A device accepts an offer until this time after its expiry (60 seconds).
pub const OFFER_EXPIRY_GRACE_MS: u64 = 60_000;

/// The lifetime of an offer (5 minutes, section 5.1).
/// The field `e` is at most this time after the creation time.
pub const OFFER_LIFETIME_MS: u64 = 300_000;

/// The maximum length of the host display name, in characters.
pub const MAX_HOST_NAME_CHARS: usize = 64;

/// The HKDF salt of the derived values (section 5.2).
pub const PAIRING_HKDF_SALT: &[u8] = b"halv-remote/pairing/v1";

/// The HKDF info of `invite_token`.
pub const INVITE_TOKEN_HKDF_INFO: &[u8] = b"relay-invite-token";

/// The HKDF info of `pairing_psk`.
pub const PSK_HKDF_INFO: &[u8] = b"noise-psk";

secret32!(
    /// The 32-byte pairing secret of an offer (field `s`).
    PairingSecret
);

secret32!(
    /// The invite token that the device sends to the relay at pairing (section 5.2).
    InviteToken
);

secret32!(
    /// The Noise PSK of the pairing handshake (section 5.2).
    ///
    /// It never leaves the two peers.
    /// The host keeps it in memory only, until the offer ends.
    Psk
);

impl PairingSecret {
    /// Makes a new random pairing secret.
    pub fn generate() -> Result<PairingSecret> {
        Ok(PairingSecret(*random_array::<32>()?))
    }

    /// Derives `invite_token`.
    pub fn invite_token(&self) -> InviteToken {
        derive_invite_token(self)
    }

    /// Derives `pairing_psk`.
    pub fn psk(&self) -> Psk {
        derive_psk(self)
    }
}

impl InviteToken {
    /// Returns the `b64u` text of the token.
    pub fn to_b64u(&self) -> Zeroizing<String> {
        b64u::encode_secret(&self.0)
    }

    /// Returns `b64u(SHA-256(invite_token))`: the `hash` of `invite_put`.
    pub fn hash(&self) -> TokenHash {
        TokenHash::of(&self.0)
    }
}

fn hkdf32(secret: &PairingSecret, info: &[u8]) -> Zeroizing<[u8; 32]> {
    let hkdf = Hkdf::<Sha256>::new(Some(PAIRING_HKDF_SALT), secret.as_bytes());
    let mut okm = Zeroizing::new([0u8; 32]);
    hkdf.expand(info, &mut okm[..])
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    okm
}

/// `invite_token = HKDF-SHA256(ikm = secret, salt = "halv-remote/pairing/v1", info = "relay-invite-token", L = 32)`
pub fn derive_invite_token(secret: &PairingSecret) -> InviteToken {
    InviteToken(*hkdf32(secret, INVITE_TOKEN_HKDF_INFO))
}

/// `pairing_psk = HKDF-SHA256(ikm = secret, salt = "halv-remote/pairing/v1", info = "noise-psk", L = 32)`
pub fn derive_psk(secret: &PairingSecret) -> Psk {
    Psk(*hkdf32(secret, PSK_HKDF_INFO))
}

/// Returns true when `url` is a valid relay base URL (field `r`).
///
/// - `wss://` is required.
///   `ws://` is allowed only for the hosts `127.0.0.1`, `::1`, and `localhost`.
/// - The URL has no trailing slash, no user info, no query, and no fragment.
/// - A port is optional. It is a decimal number from 1 to 65535.
pub fn is_valid_relay_url(url: &str) -> bool {
    let (secure, rest) = if let Some(rest) = url.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("ws://") {
        (false, rest)
    } else {
        return false;
    };
    if url.ends_with('/')
        || !rest.bytes().all(|byte| byte.is_ascii_graphic())
        || rest.contains(['?', '#', '@', '\\', '"'])
    {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or_default();
    match relay_host(authority) {
        Some(host) => secure || matches!(host, "127.0.0.1" | "::1" | "localhost"),
        None => false,
    }
}

/// Returns the host of a URL authority (`host[:port]`), without IPv6 brackets.
fn relay_host(authority: &str) -> Option<&str> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let valid = !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b':' || byte == b'.');
        if !valid {
            return None;
        }
        let port = if after.is_empty() {
            None
        } else {
            Some(after.strip_prefix(':')?)
        };
        (host, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        let valid = !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
        if !valid {
            return None;
        }
        (host, port)
    };
    if let Some(port) = port {
        let valid = (1..=5).contains(&port.len())
            && port.bytes().all(|byte| byte.is_ascii_digit())
            && matches!(port.parse::<u32>(), Ok(1..=65_535));
        if !valid {
            return None;
        }
    }
    Some(host)
}

/// A pairing offer (section 5.1).
///
/// `Debug` does not show the pairing secret.
#[derive(Clone)]
pub struct PairingOffer {
    relay_url: String,
    host_id: HostId,
    host_noise_public: [u8; 32],
    secret: PairingSecret,
    expires_at_ms: u64,
    host_name: Option<String>,
}

impl PairingOffer {
    /// Makes an offer. The relay URL and the host name must follow section 5.1.
    /// The host key must not have a low order.
    pub fn new(
        relay_url: impl Into<String>,
        host_id: HostId,
        host_noise_public: [u8; 32],
        secret: PairingSecret,
        expires_at_ms: u64,
        host_name: Option<String>,
    ) -> Result<PairingOffer> {
        let relay_url = relay_url.into();
        if !is_valid_relay_url(&relay_url) {
            return Err(Error::InvalidOffer(OfferError::RelayUrl));
        }
        if host_name
            .as_deref()
            .is_some_and(|name| name.chars().count() > MAX_HOST_NAME_CHARS)
        {
            return Err(Error::InvalidOffer(OfferError::Name));
        }
        if is_low_order_x25519(&host_noise_public) {
            return Err(Error::InvalidOffer(OfferError::HostKey));
        }
        Ok(PairingOffer {
            relay_url,
            host_id,
            host_noise_public,
            secret,
            expires_at_ms,
            host_name,
        })
    }

    /// Makes an offer for `identity` with a new pairing secret.
    /// The offer expires 5 minutes after `now_ms`.
    pub fn generate(
        identity: &HostIdentity,
        relay_url: impl Into<String>,
        host_name: Option<String>,
        now_ms: u64,
    ) -> Result<PairingOffer> {
        PairingOffer::new(
            relay_url,
            identity.host_id(),
            identity.noise_public(),
            PairingSecret::generate()?,
            now_ms.saturating_add(OFFER_LIFETIME_MS),
            host_name,
        )
    }

    /// Writes the `halv://pair?p=<b64u(JSON)>` URI.
    ///
    /// The URI holds the pairing secret. Show it only as a QR code or as text to copy. Never log it.
    pub fn to_uri(&self) -> Zeroizing<String> {
        #[derive(Serialize)]
        struct Wire<'a> {
            v: u64,
            r: &'a str,
            h: &'a str,
            k: &'a str,
            s: &'a str,
            e: u64,
            #[serde(skip_serializing_if = "Option::is_none")]
            n: Option<&'a str>,
        }

        let host_key = b64u::encode(&self.host_noise_public);
        let secret = b64u::encode_secret(self.secret.as_bytes());
        let name = self.host_name.as_deref();
        let wire = Wire {
            v: OFFER_VERSION,
            r: &self.relay_url,
            h: self.host_id.as_str(),
            k: &host_key,
            s: &secret,
            e: self.expires_at_ms,
            n: name,
        };
        // JSON escapes a character into at most 6 bytes. With this capacity the buffer never
        // moves, so no copy of the secret stays behind in freed memory.
        let capacity = 192 + 2 * self.relay_url.len() + 6 * name.map_or(0, str::len);
        let mut json = Zeroizing::new(Vec::with_capacity(capacity));
        serde_json::to_writer(&mut *json, &wire).expect("offer JSON always serializes");
        let payload = b64u::encode_secret(&json);
        let mut uri = Zeroizing::new(String::with_capacity(
            OFFER_URI_PREFIX.len() + payload.len(),
        ));
        uri.push_str(OFFER_URI_PREFIX);
        uri.push_str(&payload);
        uri
    }

    /// Reads an offer URI and checks every rule of section 5.1.
    ///
    /// Call it only for text from the in-app QR scanner or for a code that the user pastes.
    /// Never call it for a deep link, because any app can claim the `halv://` scheme.
    ///
    /// - `v` greater than 1 gives [`Error::UpdateRequired`].
    /// - An offer that expired more than 60 seconds before `now_ms` gives [`Error::OfferExpired`].
    /// - An expiry more than 5 minutes and 60 seconds after `now_ms` gives
    ///   [`OfferError::Expiry`], because an offer lasts 5 minutes at most.
    /// - A low-order host key gives [`OfferError::HostKey`].
    /// - Any other broken rule gives [`Error::InvalidOffer`].
    ///
    /// Unknown fields are ignored. A `null` name is the same as no name.
    /// Whitespace around the URI is ignored.
    pub fn parse_uri(uri: &str, now_ms: u64) -> Result<PairingOffer> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            v: Option<serde_json::Value>,
            #[serde(default)]
            r: Option<serde_json::Value>,
            #[serde(default)]
            h: Option<serde_json::Value>,
            #[serde(default)]
            k: Option<serde_json::Value>,
            #[serde(default)]
            s: Option<serde_json::Value>,
            #[serde(default)]
            e: Option<serde_json::Value>,
            #[serde(default)]
            n: Option<serde_json::Value>,
        }

        let invalid = Error::InvalidOffer;
        let payload = uri
            .trim()
            .strip_prefix(OFFER_URI_PREFIX)
            .ok_or(invalid(OfferError::Uri))?;
        let json = Zeroizing::new(b64u::decode(payload).map_err(|_| invalid(OfferError::Uri))?);
        if !crate::is_json_object(&json) {
            return Err(invalid(OfferError::Json));
        }
        let mut wire: Wire =
            serde_json::from_slice(&json).map_err(|_| invalid(OfferError::Json))?;
        let secret_text = match wire.s.take() {
            Some(serde_json::Value::String(text)) => Some(Zeroizing::new(text)),
            _ => None,
        };

        // The version comes first: a newer offer can have other fields.
        match wire.v.as_ref().and_then(serde_json::Value::as_u64) {
            Some(OFFER_VERSION) => {}
            Some(version) if version > OFFER_VERSION => {
                return Err(Error::UpdateRequired { version });
            }
            _ => return Err(invalid(OfferError::Version)),
        }

        let relay_url = match wire.r.take() {
            Some(serde_json::Value::String(url)) if is_valid_relay_url(&url) => url,
            _ => return Err(invalid(OfferError::RelayUrl)),
        };
        let host_id = wire
            .h
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .and_then(|text| HostId::parse(text).ok())
            .ok_or(invalid(OfferError::HostId))?;
        let host_noise_public = wire
            .k
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .and_then(b64u::decode_array::<32>)
            .filter(|key| !is_low_order_x25519(key))
            .ok_or(invalid(OfferError::HostKey))?;
        let secret = secret_text
            .as_deref()
            .and_then(|text| b64u::decode_array::<32>(text))
            .ok_or(invalid(OfferError::Secret))?;
        let expires_at_ms = wire
            .e
            .as_ref()
            .and_then(serde_json::Value::as_u64)
            .ok_or(invalid(OfferError::Expiry))?;
        let host_name = match wire.n.take() {
            None => None,
            Some(serde_json::Value::String(name))
                if name.chars().count() <= MAX_HOST_NAME_CHARS =>
            {
                Some(name)
            }
            Some(_) => return Err(invalid(OfferError::Name)),
        };

        if now_ms > expires_at_ms.saturating_add(OFFER_EXPIRY_GRACE_MS) {
            return Err(Error::OfferExpired);
        }
        if expires_at_ms > now_ms.saturating_add(OFFER_LIFETIME_MS + OFFER_EXPIRY_GRACE_MS) {
            return Err(invalid(OfferError::Expiry));
        }

        Ok(PairingOffer {
            relay_url,
            host_id,
            host_noise_public: *host_noise_public,
            secret: PairingSecret(*secret),
            expires_at_ms,
            host_name,
        })
    }

    /// Returns the relay base URL (field `r`).
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// Returns the host id (field `h`).
    pub fn host_id(&self) -> &HostId {
        &self.host_id
    }

    /// Returns the `host_noise` public key (field `k`).
    /// The device pins this key for the pairing and for every later session.
    pub fn host_noise_public(&self) -> &[u8; 32] {
        &self.host_noise_public
    }

    /// Returns the pairing secret (field `s`).
    pub fn secret(&self) -> &PairingSecret {
        &self.secret
    }

    /// Returns the expiry time in epoch milliseconds (field `e`).
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }

    /// Returns the host display name (field `n`).
    pub fn host_name(&self) -> Option<&str> {
        self.host_name.as_deref()
    }

    /// Derives `invite_token` from the pairing secret.
    pub fn invite_token(&self) -> InviteToken {
        derive_invite_token(&self.secret)
    }

    /// Derives `pairing_psk` from the pairing secret.
    pub fn psk(&self) -> Psk {
        derive_psk(&self.secret)
    }
}

impl fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingOffer")
            .field("relay_url", &self.relay_url)
            .field("host_id", &self.host_id)
            .field("host_noise_public", &b64u::encode(&self.host_noise_public))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("host_name", &self.host_name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_760_460_000_000;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn sample_offer() -> PairingOffer {
        PairingOffer::new(
            "wss://relay.halv.ai",
            HostId::parse("nYqEorAQnfAioCLxARz8mA").unwrap(),
            [5u8; 32],
            PairingSecret::from_bytes([0x42u8; 32]),
            NOW + OFFER_LIFETIME_MS,
            Some("Studio Mac".to_owned()),
        )
        .unwrap()
    }

    /// Builds an offer URI from JSON text.
    fn uri_of(json: &str) -> String {
        format!("{OFFER_URI_PREFIX}{}", b64u::encode(json.as_bytes()))
    }

    /// Returns the JSON object of a valid offer, so a test can change one field.
    fn valid_json() -> serde_json::Map<String, serde_json::Value> {
        let uri = sample_offer().to_uri();
        let payload = uri.strip_prefix(OFFER_URI_PREFIX).unwrap();
        serde_json::from_slice(&b64u::decode(payload).unwrap()).unwrap()
    }

    fn parse_with(field: &str, value: serde_json::Value) -> Result<PairingOffer> {
        let mut json = valid_json();
        json.insert(field.to_owned(), value);
        PairingOffer::parse_uri(&uri_of(&serde_json::to_string(&json).unwrap()), NOW)
    }

    fn parse_without(field: &str) -> Result<PairingOffer> {
        let mut json = valid_json();
        json.remove(field);
        PairingOffer::parse_uri(&uri_of(&serde_json::to_string(&json).unwrap()), NOW)
    }

    #[test]
    fn derived_values_match_vector_v2() {
        let secret = PairingSecret::from_bytes([0x42u8; 32]);
        assert_eq!(
            hex(derive_invite_token(&secret).as_bytes()),
            "70c315d483322856d11594f882598c861c3900dd734726428bb806d57c86c247"
        );
        assert_eq!(
            hex(derive_psk(&secret).as_bytes()),
            "08ae975f36336b98f5d6f415b0706f143f283d5e5d52d8d0219c8aa9e417a0e2"
        );
        assert_eq!(
            secret.invite_token().as_bytes(),
            derive_invite_token(&secret).as_bytes()
        );
        assert_eq!(secret.psk().as_bytes(), derive_psk(&secret).as_bytes());
        assert_eq!(
            secret.invite_token().hash(),
            TokenHash::of(derive_invite_token(&secret).as_bytes())
        );
    }

    #[test]
    fn uri_round_trip() {
        let offer = sample_offer();
        let uri = offer.to_uri();
        assert!(uri.starts_with("halv://pair?p="));
        let parsed = PairingOffer::parse_uri(&uri, NOW).unwrap();
        assert_eq!(parsed.relay_url(), "wss://relay.halv.ai");
        assert_eq!(parsed.host_id(), offer.host_id());
        assert_eq!(parsed.host_noise_public(), &[5u8; 32]);
        assert_eq!(parsed.secret().as_bytes(), &[0x42u8; 32]);
        assert_eq!(parsed.expires_at_ms(), NOW + OFFER_LIFETIME_MS);
        assert_eq!(parsed.host_name(), Some("Studio Mac"));
        assert_eq!(parsed.psk().as_bytes(), offer.psk().as_bytes());
        assert_eq!(
            parsed.invite_token().as_bytes(),
            offer.invite_token().as_bytes()
        );

        // Whitespace around a pasted code is ignored.
        let padded = format!("  {}\n", uri.as_str());
        assert!(PairingOffer::parse_uri(&padded, NOW).is_ok());
    }

    #[test]
    fn offer_json_has_the_contract_fields() {
        let json = valid_json();
        let mut keys: Vec<&str> = json.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["e", "h", "k", "n", "r", "s", "v"]);
        assert_eq!(json["v"], 1);
        assert_eq!(json["e"], NOW + OFFER_LIFETIME_MS);
        assert_eq!(json["k"], b64u::encode(&[5u8; 32]));
        assert_eq!(json["s"], b64u::encode(&[0x42u8; 32]));
    }

    #[test]
    fn name_is_optional() {
        let mut offer = sample_offer();
        offer.host_name = None;
        let parsed = PairingOffer::parse_uri(&offer.to_uri(), NOW).unwrap();
        assert_eq!(parsed.host_name(), None);
        assert_eq!(valid_json().len(), 7);
        assert!(parse_without("n").is_ok());
    }

    #[test]
    fn generated_offer_lasts_five_minutes() {
        let identity = HostIdentity::generate_at(NOW).unwrap();
        let offer = PairingOffer::generate(&identity, "wss://relay.halv.ai", None, NOW).unwrap();
        assert_eq!(offer.expires_at_ms(), NOW + 300_000);
        assert_eq!(offer.host_id(), &identity.host_id());
        assert_eq!(offer.host_noise_public(), &identity.noise_public());
        let other = PairingOffer::generate(&identity, "wss://relay.halv.ai", None, NOW).unwrap();
        assert_ne!(offer.secret().as_bytes(), other.secret().as_bytes());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        assert!(parse_with("x", serde_json::json!({"nested": [1, 2]})).is_ok());
    }

    #[test]
    fn rejects_bad_uri_text() {
        let bad = |uri: &str| PairingOffer::parse_uri(uri, NOW).unwrap_err();
        assert_eq!(bad(""), Error::InvalidOffer(OfferError::Uri));
        assert_eq!(bad("https://halv.ai"), Error::InvalidOffer(OfferError::Uri));
        assert_eq!(
            bad("halv://pair?q=abc"),
            Error::InvalidOffer(OfferError::Uri)
        );
        assert_eq!(
            bad("halv://pair?p=a+b/"),
            Error::InvalidOffer(OfferError::Uri)
        );
        assert_eq!(
            bad("halv://pair?p=AAAA="),
            Error::InvalidOffer(OfferError::Uri)
        );
        assert_eq!(bad(&uri_of("[1,2]")), Error::InvalidOffer(OfferError::Json));
        assert_eq!(
            bad(&uri_of("not json")),
            Error::InvalidOffer(OfferError::Json)
        );
        assert_eq!(
            bad(&uri_of("\"text\"")),
            Error::InvalidOffer(OfferError::Json)
        );
        assert_eq!(
            bad(&uri_of("{\"v\":1")),
            Error::InvalidOffer(OfferError::Json)
        );
        assert_eq!(
            bad(&uri_of("{\"v\":1,\"v\":1}")),
            Error::InvalidOffer(OfferError::Json)
        );
    }

    #[test]
    fn rejects_each_invalid_field() {
        use serde_json::json;
        let cases: Vec<(&str, serde_json::Value, OfferError)> = vec![
            ("v", json!(0), OfferError::Version),
            ("v", json!("1"), OfferError::Version),
            ("v", json!(1.5), OfferError::Version),
            ("v", json!(-1), OfferError::Version),
            ("r", json!("https://relay.halv.ai"), OfferError::RelayUrl),
            ("r", json!("wss://relay.halv.ai/"), OfferError::RelayUrl),
            ("r", json!("ws://relay.halv.ai"), OfferError::RelayUrl),
            ("r", json!("ws://10.0.0.2:8787"), OfferError::RelayUrl),
            (
                "r",
                json!("ws://127.0.0.1.example.com"),
                OfferError::RelayUrl,
            ),
            ("r", json!("wss://user@relay.halv.ai"), OfferError::RelayUrl),
            ("r", json!("wss://relay.halv.ai?x=1"), OfferError::RelayUrl),
            ("r", json!("wss://relay.halv.ai:0"), OfferError::RelayUrl),
            (
                "r",
                json!("wss://relay.halv.ai:65536"),
                OfferError::RelayUrl,
            ),
            ("r", json!("wss://"), OfferError::RelayUrl),
            ("r", json!(5), OfferError::RelayUrl),
            ("h", json!("nYqEorAQnfAioCLxARz8m"), OfferError::HostId),
            ("h", json!("nYqEorAQnfAioCLxARz8mAA"), OfferError::HostId),
            ("h", json!("nYqEorAQnfAioCLxARz8m+"), OfferError::HostId),
            ("h", json!(null), OfferError::HostId),
            ("k", json!(b64u::encode(&[5u8; 31])), OfferError::HostKey),
            ("k", json!(b64u::encode(&[5u8; 33])), OfferError::HostKey),
            (
                "k",
                json!(format!("{}=", b64u::encode(&[5u8; 32]))),
                OfferError::HostKey,
            ),
            // Low-order keys: u = 0 and u = 1.
            ("k", json!(b64u::encode(&[0u8; 32])), OfferError::HostKey),
            (
                "k",
                json!(b64u::encode(&{
                    let mut key = [0u8; 32];
                    key[0] = 1;
                    key
                })),
                OfferError::HostKey,
            ),
            ("s", json!(b64u::encode(&[1u8; 16])), OfferError::Secret),
            (
                "s",
                json!(b64u::encode(&[1u8; 32]).replace('B', "+")),
                OfferError::Secret,
            ),
            ("s", json!(42), OfferError::Secret),
            ("e", json!(-5), OfferError::Expiry),
            ("e", json!("1760460300000"), OfferError::Expiry),
            ("e", json!(1.5e12), OfferError::Expiry),
            ("n", json!("x".repeat(65)), OfferError::Name),
            ("n", json!(7), OfferError::Name),
            ("n", json!(["name"]), OfferError::Name),
        ];
        for (field, value, expected) in cases {
            let shown = value.to_string();
            assert_eq!(
                parse_with(field, value).unwrap_err(),
                Error::InvalidOffer(expected),
                "field {field} = {shown}"
            );
        }
        for (field, expected) in [
            ("v", OfferError::Version),
            ("r", OfferError::RelayUrl),
            ("h", OfferError::HostId),
            ("k", OfferError::HostKey),
            ("s", OfferError::Secret),
            ("e", OfferError::Expiry),
        ] {
            assert_eq!(
                parse_without(field).unwrap_err(),
                Error::InvalidOffer(expected),
                "missing field {field}"
            );
        }
    }

    #[test]
    fn accepts_valid_field_edges() {
        use serde_json::json;
        for url in [
            "wss://relay.halv.ai",
            "wss://relay.halv.ai:8443",
            "wss://relay.halv.ai/base",
            "ws://127.0.0.1:8787",
            "ws://localhost:8787",
            "ws://[::1]:8787",
            "ws://localhost",
        ] {
            assert!(parse_with("r", json!(url)).is_ok(), "{url}");
        }
        // 64 characters is the limit, also for characters of several bytes.
        assert!(parse_with("n", json!("é".repeat(64))).is_ok());
        assert!(parse_with("n", json!("")).is_ok());
        assert_eq!(parse_with("n", json!(null)).unwrap().host_name(), None);
    }

    #[test]
    fn newer_version_asks_for_an_update() {
        use serde_json::json;
        assert_eq!(
            parse_with("v", json!(2)).unwrap_err(),
            Error::UpdateRequired { version: 2 }
        );
        // A newer offer can change other fields, so the version check comes first.
        let uri = uri_of(r#"{"v":3,"future":true}"#);
        assert_eq!(
            PairingOffer::parse_uri(&uri, NOW).unwrap_err(),
            Error::UpdateRequired { version: 3 }
        );
    }

    #[test]
    fn expiry_has_a_grace_of_60_seconds() {
        use serde_json::json;
        let at = |expiry: u64| parse_with("e", json!(expiry));
        assert!(at(NOW).is_ok());
        assert!(at(NOW - OFFER_EXPIRY_GRACE_MS).is_ok());
        assert_eq!(
            at(NOW - OFFER_EXPIRY_GRACE_MS - 1).unwrap_err(),
            Error::OfferExpired
        );
        assert_eq!(at(0).unwrap_err(), Error::OfferExpired);
    }

    #[test]
    fn expiry_cannot_be_further_than_an_offer_lasts() {
        use serde_json::json;
        let at = |expiry: u64| parse_with("e", json!(expiry));
        let limit = NOW + OFFER_LIFETIME_MS + OFFER_EXPIRY_GRACE_MS;
        assert!(at(limit).is_ok());
        assert_eq!(
            at(limit + 1).unwrap_err(),
            Error::InvalidOffer(OfferError::Expiry)
        );
        assert_eq!(
            at(u64::MAX).unwrap_err(),
            Error::InvalidOffer(OfferError::Expiry)
        );
    }

    #[test]
    fn new_checks_the_rules() {
        let host_id = HostId::parse("nYqEorAQnfAioCLxARz8mA").unwrap();
        let secret = || PairingSecret::from_bytes([1u8; 32]);
        assert_eq!(
            PairingOffer::new(
                "ws://relay.halv.ai",
                host_id.clone(),
                [0; 32],
                secret(),
                0,
                None
            )
            .unwrap_err(),
            Error::InvalidOffer(OfferError::RelayUrl)
        );
        assert_eq!(
            PairingOffer::new(
                "wss://relay.halv.ai",
                host_id.clone(),
                [5; 32],
                secret(),
                0,
                Some("n".repeat(65))
            )
            .unwrap_err(),
            Error::InvalidOffer(OfferError::Name)
        );
        assert_eq!(
            PairingOffer::new("wss://relay.halv.ai", host_id, [0; 32], secret(), 0, None)
                .unwrap_err(),
            Error::InvalidOffer(OfferError::HostKey)
        );
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let offer = sample_offer();
        let debug = format!(
            "{offer:?} {:?} {:?} {:?}",
            offer.secret(),
            offer.psk(),
            offer.invite_token()
        );
        assert!(!debug.contains(&b64u::encode(&[0x42u8; 32])));
        assert!(!debug.contains("66, 66"));
        assert!(debug.contains("PairingSecret(<redacted>)"));
        assert!(debug.contains("Psk(<redacted>)"));
        assert!(debug.contains("InviteToken(<redacted>)"));
    }
}
