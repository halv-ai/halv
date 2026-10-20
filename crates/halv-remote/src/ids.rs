//! The host id (section 4.2) and the other ids (section 4.4).

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::b64u;
use crate::error::{Error, Result};
use crate::secret::random_array;

/// A connection id: a `u32` that is not zero (section 4.4).
///
/// The host object chooses it. It is unique among the open device sockets of one host object.
/// Serde rejects the value 0.
pub type ConnId = NonZeroU32;

/// A request id: a `u32` that is not zero (section 4.4).
///
/// The device chooses it. It is unique among the open requests of one session.
/// Serde rejects the value 0.
pub type RequestId = NonZeroU32;

/// The text length of an id: the `b64u` text of 16 bytes.
pub const ID_LEN: usize = 22;

/// The domain prefix of the host id hash (section 4.2).
pub const HOST_ID_DOMAIN: &[u8] = b"halv-remote/host-id/v1";

/// Returns true when `text` matches `^[A-Za-z0-9_-]{22}$`.
pub fn is_valid_id(text: &str) -> bool {
    text.len() == ID_LEN
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Parses an id that matches `^[A-Za-z0-9_-]{22}$`.
            pub fn parse(text: &str) -> Result<Self> {
                if is_valid_id(text) {
                    Ok(Self(text.to_owned()))
                } else {
                    Err(Error::Invalid($what))
                }
            }

            /// Returns the id text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(text: &str) -> Result<Self> {
                Self::parse(text)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                if is_valid_id(&text) {
                    Ok(Self(text))
                } else {
                    Err(serde::de::Error::custom(concat!("invalid ", $what)))
                }
            }
        }
    };
}

id_type!(
    /// A host id: the name of the host object in the relay (section 4.2).
    HostId,
    "host id"
);

id_type!(
    /// A device id. The host makes it at pairing (section 4.4).
    DeviceId,
    "device id"
);

id_type!(
    /// An invite id. The host makes one for each pairing offer (section 4.4).
    InviteId,
    "invite id"
);

impl HostId {
    /// Derives the host id from the Ed25519 relay public key (section 4.2):
    ///
    /// `b64u(SHA-256("halv-remote/host-id/v1" || 0x00 || host_relay_public)[0..16])`
    pub fn from_relay_public(relay_public: &[u8; 32]) -> HostId {
        let mut hasher = Sha256::new();
        hasher.update(HOST_ID_DOMAIN);
        hasher.update([0u8]);
        hasher.update(relay_public);
        let digest = hasher.finalize();
        HostId(b64u::encode(&digest[..16]))
    }
}

impl DeviceId {
    /// Makes a new random device id.
    pub fn generate() -> Result<DeviceId> {
        Ok(DeviceId(b64u::encode(&random_array::<16>()?[..])))
    }
}

impl InviteId {
    /// Makes a new random invite id.
    pub fn generate() -> Result<InviteId> {
        Ok(InviteId(b64u::encode(&random_array::<16>()?[..])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_rules() {
        assert!(is_valid_id("nYqEorAQnfAioCLxARz8mA"));
        assert!(is_valid_id("______________________"));
        assert!(!is_valid_id("nYqEorAQnfAioCLxARz8m"));
        assert!(!is_valid_id("nYqEorAQnfAioCLxARz8mAA"));
        assert!(!is_valid_id("nYqEorAQnfAioCLxARz8m+"));
        assert!(!is_valid_id("nYqEorAQnfAioCLxARz8m="));
        assert!(!is_valid_id("nYqEorAQnfAioCLxARz8mé"));
        assert!(HostId::parse("short").is_err());
    }

    #[test]
    fn generated_ids_are_valid_and_differ() {
        let first = DeviceId::generate().unwrap();
        let second = DeviceId::generate().unwrap();
        assert!(is_valid_id(first.as_str()));
        assert_ne!(first, second);
        assert!(is_valid_id(InviteId::generate().unwrap().as_str()));
    }

    #[test]
    fn host_id_matches_vector_v1() {
        let public: [u8; 32] = std::array::from_fn(|index| index as u8);
        assert_eq!(
            HostId::from_relay_public(&public).as_str(),
            "nYqEorAQnfAioCLxARz8mA"
        );
    }

    #[test]
    fn numeric_ids_reject_zero() {
        assert!(serde_json::from_str::<ConnId>("0").is_err());
        assert_eq!(serde_json::from_str::<ConnId>("7").unwrap().get(), 7);
        assert!(serde_json::from_str::<RequestId>("4294967296").is_err());
    }

    #[test]
    fn ids_serialize_as_text_and_validate_on_decode() {
        let id = DeviceId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"AAAAAAAAAAAAAAAAAAAAAA\""
        );
        assert!(serde_json::from_str::<DeviceId>("\"AAAA\"").is_err());
        assert_eq!(
            serde_json::from_str::<DeviceId>("\"AAAAAAAAAAAAAAAAAAAAAA\"").unwrap(),
            id
        );
    }
}
