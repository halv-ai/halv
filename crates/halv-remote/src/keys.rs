//! Host and device keys (section 4).

use std::fmt;
use std::str::FromStr;

use ed25519_dalek::{Signer as _, SigningKey};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::b64u;
use crate::error::{Error, Result};
use crate::ids::HostId;
use crate::secret::{random_array, secret32, SecretText};

/// The keychain service of the host identity (section 4.1).
pub const KEYCHAIN_SERVICE: &str = "Halv";

/// The keychain account of the host identity in release builds.
pub const KEYCHAIN_ACCOUNT: &str = "remote-identity";

/// The keychain account of the host identity in debug builds.
pub const KEYCHAIN_ACCOUNT_DEV: &str = "remote-identity-dev";

/// The version of the keychain JSON (section 4.1).
pub const IDENTITY_JSON_VERSION: u64 = 1;

/// Returns the X25519 public key of a secret key.
///
/// The key is clamped as in RFC 7748. `snow` clamps in the same way.
pub(crate) fn x25519_public(secret: &[u8; 32]) -> [u8; 32] {
    let secret = x25519_dalek::StaticSecret::from(*secret);
    x25519_dalek::PublicKey::from(&secret).to_bytes()
}

/// Returns true when an X25519 public key has a low order.
///
/// A Diffie-Hellman with a low-order key gives 32 zero bytes for every secret key,
/// so the key authenticates nothing. A key that comes from a secret key never has a low order.
/// The offer parser and the handshakes reject low-order static keys.
pub fn is_low_order_x25519(public: &[u8; 32]) -> bool {
    // Any probe value works. Clamping makes the scalar a multiple of the cofactor 8,
    // so the result is zero for the low-order points and only for them.
    let probe = x25519_dalek::StaticSecret::from([0x01; 32]);
    !probe
        .diffie_hellman(&x25519_dalek::PublicKey::from(*public))
        .was_contributory()
}

/// `b64u(SHA-256(token))`: the hash of a relay token or an invite token (section 4.3).
///
/// The relay stores only this hash. The text has 43 characters.
/// A hash is not secret, but it is a different type from a token,
/// so a caller cannot send a token where the relay expects a hash.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenHash(String);

impl TokenHash {
    /// Hashes a token.
    pub fn of(token: &[u8]) -> TokenHash {
        TokenHash(b64u::encode(&Sha256::digest(token)))
    }

    /// Parses the `b64u` text of a 32-byte hash.
    pub fn parse(text: &str) -> Result<TokenHash> {
        if b64u::decode_array::<32>(text).is_some() {
            Ok(TokenHash(text.to_owned()))
        } else {
            Err(Error::Invalid("token hash"))
        }
    }

    /// Returns the hash text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for TokenHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenHash({})", self.0)
    }
}

impl FromStr for TokenHash {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        TokenHash::parse(text)
    }
}

impl Serialize for TokenHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TokenHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        TokenHash::parse(&text).map_err(|_| de::Error::custom("invalid token hash"))
    }
}

/// The long-term keys of a host (section 4.1).
///
/// - `host_noise`: the X25519 Noise static key of the responder.
/// - `host_relay`: the Ed25519 key for the relay host authentication.
///
/// The secrets zeroize on drop. `Debug` shows only the public values.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct HostIdentity {
    noise_secret: [u8; 32],
    noise_public: [u8; 32],
    relay_seed: [u8; 32],
    relay_public: [u8; 32],
    created_at_ms: u64,
}

impl HostIdentity {
    /// Makes a new identity. `createdAt` is the current time.
    ///
    /// Call it only when the keychain has no identity item.
    /// After a keychain read error, or when the stored JSON does not parse,
    /// do not make a new identity: a new identity cuts off every paired device (section 4.1).
    pub fn generate() -> Result<HostIdentity> {
        HostIdentity::generate_at(crate::unix_time_ms())
    }

    /// Makes a new identity with the time `created_at_ms` (epoch milliseconds).
    ///
    /// The rule of [`HostIdentity::generate`] applies.
    pub fn generate_at(created_at_ms: u64) -> Result<HostIdentity> {
        let noise = random_array::<32>()?;
        let relay = random_array::<32>()?;
        Ok(HostIdentity::from_secrets(*noise, *relay, created_at_ms))
    }

    /// Makes an identity from an X25519 secret key and an Ed25519 seed.
    pub fn from_secrets(
        noise_secret: [u8; 32],
        relay_seed: [u8; 32],
        created_at_ms: u64,
    ) -> HostIdentity {
        let relay_public = SigningKey::from_bytes(&relay_seed)
            .verifying_key()
            .to_bytes();
        HostIdentity {
            noise_public: x25519_public(&noise_secret),
            noise_secret,
            relay_seed,
            relay_public,
            created_at_ms,
        }
    }

    /// Reads the keychain JSON of section 4.1.
    ///
    /// Unknown fields are ignored. A version other than 1 fails.
    /// A failure here is not a missing item: the gateway stays off and reports the error.
    pub fn from_keychain_json(text: &str) -> Result<HostIdentity> {
        #[derive(Deserialize)]
        struct Stored {
            v: u64,
            noise: SecretText,
            relay: SecretText,
            #[serde(rename = "createdAt")]
            created_at: u64,
        }

        if !crate::is_json_object(text.as_bytes()) {
            return Err(Error::Invalid("host identity JSON"));
        }
        let stored: Stored =
            serde_json::from_str(text).map_err(|_| Error::Invalid("host identity JSON"))?;
        if stored.v != IDENTITY_JSON_VERSION {
            return Err(Error::Invalid("host identity version"));
        }
        let noise = b64u::decode_array::<32>(&stored.noise.0)
            .ok_or(Error::Invalid("host identity Noise key"))?;
        let relay = b64u::decode_array::<32>(&stored.relay.0)
            .ok_or(Error::Invalid("host identity relay key"))?;
        Ok(HostIdentity::from_secrets(
            *noise,
            *relay,
            stored.created_at,
        ))
    }

    /// Writes the keychain JSON of section 4.1.
    ///
    /// The text holds both secrets. Store it only in the keychain.
    pub fn to_keychain_json(&self) -> Zeroizing<String> {
        #[derive(Serialize)]
        struct Stored<'a> {
            v: u64,
            noise: &'a str,
            relay: &'a str,
            #[serde(rename = "createdAt")]
            created_at: u64,
        }

        let noise = b64u::encode_secret(&self.noise_secret);
        let relay = b64u::encode_secret(&self.relay_seed);
        let stored = Stored {
            v: IDENTITY_JSON_VERSION,
            noise: &noise,
            relay: &relay,
            created_at: self.created_at_ms,
        };
        // The capacity holds the whole text, so the buffer never moves and leaves no copy behind.
        let mut json = Vec::with_capacity(256);
        serde_json::to_writer(&mut json, &stored).expect("identity JSON always serializes");
        Zeroizing::new(String::from_utf8(json).expect("JSON is UTF-8"))
    }

    /// Returns the host id (section 4.2).
    pub fn host_id(&self) -> HostId {
        HostId::from_relay_public(&self.relay_public)
    }

    /// Returns the X25519 public key of `host_noise`.
    pub fn noise_public(&self) -> [u8; 32] {
        self.noise_public
    }

    /// Returns the Ed25519 public key of `host_relay`.
    pub fn relay_public(&self) -> [u8; 32] {
        self.relay_public
    }

    /// Returns the creation time in epoch milliseconds.
    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }

    pub(crate) fn noise_secret(&self) -> &[u8; 32] {
        &self.noise_secret
    }

    /// Signs `message` with the Ed25519 relay key.
    pub(crate) fn sign_relay(&self, message: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(&self.relay_seed)
            .sign(message)
            .to_bytes()
    }
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostIdentity")
            .field("host_id", &self.host_id())
            .field("noise_public", &b64u::encode(&self.noise_public))
            .field("relay_public", &b64u::encode(&self.relay_public))
            .field("created_at_ms", &self.created_at_ms)
            .finish_non_exhaustive()
    }
}

secret32!(
    /// The 32-byte relay token of a device (section 4.3).
    ///
    /// The device sends it as `Authorization: Bearer <b64u token>`.
    /// The relay and the host store only its hash.
    /// CBOR encodes it as a byte string (`token: bytes(32)` of `pair.request`).
    RelayToken
);

impl RelayToken {
    /// Makes a new random relay token.
    pub fn generate() -> Result<RelayToken> {
        Ok(RelayToken(*random_array::<32>()?))
    }

    /// Returns the `b64u` text of the token.
    pub fn to_b64u(&self) -> Zeroizing<String> {
        b64u::encode_secret(&self.0)
    }

    /// Reads a token from its `b64u` text.
    pub fn from_b64u(text: &str) -> Result<RelayToken> {
        b64u::decode_array::<32>(text)
            .map(|bytes| RelayToken(*bytes))
            .ok_or(Error::Invalid("relay token"))
    }

    /// Returns `b64u(SHA-256(token))`.
    pub fn hash(&self) -> TokenHash {
        TokenHash::of(&self.0)
    }
}

impl Serialize for RelayToken {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for RelayToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TokenVisitor;

        impl Visitor<'_> for TokenVisitor {
            type Value = RelayToken;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte string of 32 bytes")
            }

            fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<RelayToken, E> {
                let array: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| E::invalid_length(bytes.len(), &self))?;
                Ok(RelayToken(array))
            }

            fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<RelayToken, E> {
                let bytes = Zeroizing::new(bytes);
                self.visit_bytes(&bytes)
            }
        }

        deserializer.deserialize_bytes(TokenVisitor)
    }
}

/// The keys that a device makes for one paired host (section 4.3).
///
/// - `device_noise`: an X25519 key pair, the Noise static key of the initiator.
/// - `relay_token`: 32 random bytes for the relay admission.
///
/// The device keeps both secrets in the iOS Keychain or the Android Keystore,
/// in "this device only" items that do not sync to other devices or backups.
/// The secrets zeroize on drop. `Debug` shows only the public key.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct DeviceKeys {
    noise_secret: [u8; 32],
    noise_public: [u8; 32],
    relay_token: RelayToken,
}

impl DeviceKeys {
    /// Makes a new X25519 key pair and a new relay token.
    pub fn generate() -> Result<DeviceKeys> {
        let noise = random_array::<32>()?;
        Ok(DeviceKeys::from_parts(*noise, RelayToken::generate()?))
    }

    /// Makes the keys from a stored X25519 secret key and a stored relay token.
    pub fn from_parts(noise_secret: [u8; 32], relay_token: RelayToken) -> DeviceKeys {
        DeviceKeys {
            noise_public: x25519_public(&noise_secret),
            noise_secret,
            relay_token,
        }
    }

    /// Returns the X25519 public key of `device_noise`.
    pub fn noise_public(&self) -> [u8; 32] {
        self.noise_public
    }

    /// Returns the X25519 secret key of `device_noise`.
    /// Store it only in the iOS Keychain or the Android Keystore. Never log it.
    pub fn noise_secret(&self) -> &[u8; 32] {
        &self.noise_secret
    }

    /// Returns the relay token.
    pub fn relay_token(&self) -> &RelayToken {
        &self.relay_token
    }

    /// Returns `token_hash = b64u(SHA-256(relay_token))`.
    pub fn token_hash(&self) -> TokenHash {
        self.relay_token.hash()
    }
}

impl fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("noise_public", &b64u::encode(&self.noise_public))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_matches_vector_v3() {
        let token: [u8; 32] = std::array::from_fn(|index| 0x20 + index as u8);
        let expected = "ctu3M2x2eAAj-D2kw1Xy7uqFczsT00d2l5F3kMEikIQ";
        assert_eq!(TokenHash::of(&token).as_str(), expected);
        assert_eq!(RelayToken::from_bytes(token).hash().as_str(), expected);
        let device = DeviceKeys::from_parts([9u8; 32], RelayToken::from_bytes(token));
        assert_eq!(device.token_hash().as_str(), expected);
    }

    #[test]
    fn token_hash_text_is_validated() {
        let hash = TokenHash::of(b"token");
        assert_eq!(TokenHash::parse(hash.as_str()).unwrap(), hash);
        assert!(TokenHash::parse("short").is_err());
        assert!(TokenHash::parse(&format!("{hash}=")).is_err());
        assert!(serde_json::from_str::<TokenHash>("\"abc\"").is_err());
        let json = serde_json::to_string(&hash).unwrap();
        assert_eq!(serde_json::from_str::<TokenHash>(&json).unwrap(), hash);
    }

    #[test]
    fn keychain_json_round_trip() {
        let identity = HostIdentity::generate_at(1_760_460_000_000).unwrap();
        let json = identity.to_keychain_json();
        assert!(json.starts_with("{\"v\":1,\"noise\":\""));
        assert!(json.ends_with(",\"createdAt\":1760460000000}"));

        let restored = HostIdentity::from_keychain_json(&json).unwrap();
        assert_eq!(restored.noise_public(), identity.noise_public());
        assert_eq!(restored.relay_public(), identity.relay_public());
        assert_eq!(restored.host_id(), identity.host_id());
        assert_eq!(restored.created_at_ms(), 1_760_460_000_000);
    }

    #[test]
    fn keychain_json_has_the_exact_format() {
        let identity = HostIdentity::from_secrets([1u8; 32], [2u8; 32], 1_760_460_000_000);
        let expected = format!(
            "{{\"v\":1,\"noise\":\"{}\",\"relay\":\"{}\",\"createdAt\":1760460000000}}",
            b64u::encode(&[1u8; 32]),
            b64u::encode(&[2u8; 32])
        );
        assert_eq!(identity.to_keychain_json().as_str(), expected);
    }

    #[test]
    fn keychain_json_rules() {
        let key = b64u::encode(&[7u8; 32]);
        let valid = format!(
            "{{\"v\":1,\"noise\":\"{key}\",\"relay\":\"{key}\",\"createdAt\":5,\"extra\":true}}"
        );
        assert!(HostIdentity::from_keychain_json(&valid).is_ok());

        let wrong_version = valid.replace("\"v\":1", "\"v\":2");
        assert_eq!(
            HostIdentity::from_keychain_json(&wrong_version).unwrap_err(),
            Error::Invalid("host identity version")
        );
        let short_key = valid.replacen(&key, &key[..42], 1);
        assert!(HostIdentity::from_keychain_json(&short_key).is_err());
        let missing_time = valid.replace(",\"createdAt\":5", "");
        assert!(HostIdentity::from_keychain_json(&missing_time).is_err());
        assert!(HostIdentity::from_keychain_json("not json").is_err());
        assert!(HostIdentity::from_keychain_json("").is_err());
        let array = format!("[1,\"{key}\",\"{key}\",5]");
        assert!(HostIdentity::from_keychain_json(&array).is_err());
    }

    fn unhex32(text: &str) -> [u8; 32] {
        std::array::from_fn(|index| {
            u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap()
        })
    }

    #[test]
    fn low_order_keys_are_detected() {
        // The low-order points of Curve25519 and its twist, with non-canonical encodings.
        for text in [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0100000000000000000000000000000000000000000000000000000000000000",
            "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
            "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ] {
            assert!(is_low_order_x25519(&unhex32(text)), "{text}");
        }
        // The base point (u = 9) and keys from secret keys have a large order.
        let mut base_point = [0u8; 32];
        base_point[0] = 9;
        assert!(!is_low_order_x25519(&base_point));
        assert!(!is_low_order_x25519(&x25519_public(&[0x42; 32])));
        assert!(!is_low_order_x25519(
            &DeviceKeys::generate().unwrap().noise_public()
        ));
        assert!(!is_low_order_x25519(
            &HostIdentity::generate().unwrap().noise_public()
        ));
    }

    #[test]
    fn public_keys_match_the_secrets() {
        let identity = HostIdentity::from_secrets([1u8; 32], [2u8; 32], 0);
        assert_eq!(identity.noise_public(), x25519_public(&[1u8; 32]));
        let signing = SigningKey::from_bytes(&[2u8; 32]);
        assert_eq!(identity.relay_public(), signing.verifying_key().to_bytes());
        assert_eq!(
            identity.host_id(),
            HostId::from_relay_public(&signing.verifying_key().to_bytes())
        );
    }

    #[test]
    fn debug_output_is_redacted() {
        let identity = HostIdentity::from_secrets([0xabu8; 32], [0xcdu8; 32], 0);
        let secret_text = b64u::encode(&[0xabu8; 32]);
        let seed_text = b64u::encode(&[0xcdu8; 32]);
        let debug = format!("{identity:?}");
        assert!(!debug.contains(&secret_text));
        assert!(!debug.contains(&seed_text));

        let device = DeviceKeys::from_parts([0xabu8; 32], RelayToken::from_bytes([0xcdu8; 32]));
        let debug = format!("{device:?} {:?}", device.relay_token());
        assert!(!debug.contains(&secret_text));
        assert!(!debug.contains(&seed_text));
        assert!(!debug.contains("171"));
        assert!(debug.contains("RelayToken(<redacted>)"));
    }

    #[test]
    fn relay_token_text_and_hash() {
        let token = RelayToken::generate().unwrap();
        let text = token.to_b64u();
        assert_eq!(text.len(), 43);
        let parsed = RelayToken::from_b64u(&text).unwrap();
        assert_eq!(parsed.as_bytes(), token.as_bytes());
        assert_eq!(parsed.hash(), TokenHash::of(token.as_bytes()));
        assert!(RelayToken::from_b64u("short").is_err());
    }

    #[test]
    fn generated_keys_differ() {
        let first = DeviceKeys::generate().unwrap();
        let second = DeviceKeys::generate().unwrap();
        assert_ne!(first.noise_public(), second.noise_public());
        assert_ne!(first.token_hash(), second.token_hash());
        assert_ne!(
            HostIdentity::generate().unwrap().host_id(),
            HostIdentity::generate().unwrap().host_id()
        );
    }
}
