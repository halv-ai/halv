//! Lowercase hex text of digest bytes.
//!
//! sha2 0.11 digests do not implement `LowerHex`, so `format!("{:x}", ..)`
//! no longer works on them. This helper gives the same text that sha2 0.10
//! gave: two lowercase hex digits per byte, with no separator.

use std::fmt::Write as _;

pub(crate) fn lower_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::lower_hex;

    #[test]
    fn writes_two_lowercase_digits_per_byte() {
        assert_eq!(lower_hex(&[]), "");
        assert_eq!(lower_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    // The values come from `format!("{:x}", Sha256::digest(..))` with sha2
    // 0.10.9, the version before the bump. Stored hashes (checksums, project
    // hashes, approval and snapshot hashes) must keep this exact text.
    #[test]
    fn sha256_text_matches_sha2_0_10() {
        assert_eq!(
            lower_hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            lower_hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut hasher = Sha256::new();
        hasher.update(b"a");
        hasher.update(b"bc");
        assert_eq!(
            lower_hex(&hasher.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
