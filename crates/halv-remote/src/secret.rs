//! Helpers for secret values: random bytes, redacted types, and secret text.

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Fills `N` bytes from the random number generator of the operating system.
pub(crate) fn random_array<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut out = Zeroizing::new([0u8; N]);
    getrandom::fill(&mut out[..]).map_err(|_| Error::Random)?;
    Ok(out)
}

/// Text that zeroizes on drop. JSON decoders use it for secret fields.
pub(crate) struct SecretText(pub(crate) Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| SecretText(Zeroizing::new(text)))
    }
}

/// Declares a 32-byte secret type.
///
/// The type zeroizes on drop and its `Debug` output is redacted.
macro_rules! secret32 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
        pub struct $name([u8; 32]);

        impl $name {
            /// Makes the value from 32 bytes.
            pub fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            /// Returns the secret bytes. Never log them.
            pub fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
    };
}

pub(crate) use secret32;
