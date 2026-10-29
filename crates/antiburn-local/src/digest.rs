//! Stable SHA-256 digests for local evidence and snapshot identities.

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).as_slice())
}

pub(crate) fn digest_hex(digest: &[u8]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn digest_bytes_use_lowercase_hex_with_leading_zeroes() {
        assert_eq!(digest_hex(&[]), "");
        assert_eq!(digest_hex(&[0, 1, 15, 16, 127, 255]), "00010f107fff");
    }
}
