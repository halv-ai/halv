//! `b64u`: base64url without padding (RFC 4648, section 5).
//!
//! The decoder rejects padding, characters outside the URL-safe alphabet,
//! and non-canonical trailing bits.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Encodes bytes as `b64u` text.
pub fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes `b64u` text.
pub fn decode(text: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| Error::Invalid("base64url text"))
}

/// Returns the length of the `b64u` text of `byte_len` bytes.
pub const fn encoded_len(byte_len: usize) -> usize {
    (byte_len * 4).div_ceil(3)
}

/// Decodes `b64u` text of exactly `N` bytes into a buffer that zeroizes on drop.
pub(crate) fn decode_array<const N: usize>(text: &str) -> Option<Zeroizing<[u8; N]>> {
    if text.len() != encoded_len(N) {
        return None;
    }
    let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(text).ok()?);
    if bytes.len() != N {
        return None;
    }
    let mut out = Zeroizing::new([0u8; N]);
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Encodes secret bytes into text that zeroizes on drop.
pub(crate) fn encode_secret(bytes: &[u8]) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(encoded_len(bytes.len())));
    URL_SAFE_NO_PAD.encode_string(bytes, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_lengths_match_the_contract() {
        assert_eq!(encoded_len(16), 22);
        assert_eq!(encoded_len(32), 43);
        assert_eq!(encoded_len(64), 86);
    }

    #[test]
    fn decoder_is_strict() {
        let text = encode(&[0xffu8; 32]);
        assert!(decode_array::<32>(&text).is_some());
        // Padding, the standard alphabet, and a wrong length fail.
        assert!(decode_array::<32>(&format!("{text}=")).is_none());
        assert!(decode_array::<32>(&text.replace('_', "/")).is_none());
        assert!(decode_array::<32>(&text[..42]).is_none());
        // The last character of 32 bytes carries 4 bits. Its 2 low bits must be zero.
        let mut non_canonical = encode(&[0u8; 32]);
        non_canonical.pop();
        non_canonical.push('B');
        assert!(decode_array::<32>(&non_canonical).is_none());
        assert!(decode("a+b/").is_err());
    }
}
