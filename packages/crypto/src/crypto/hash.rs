//! SHA-256, double SHA-256 and RIPEMD-160.

use ripemd::Ripemd160;
use sha2::{Digest, Sha256};

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-256 of the concatenation of `parts`, without building the concatenation.
pub fn sha256_concat<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// SHA-256 of the SHA-256 of `data` (Base58Check checksums).
pub fn hash256(data: &[u8]) -> [u8; 32] {
    sha256(&sha256(data))
}

/// RIPEMD-160 of `data` (addresses).
pub fn ripemd160(data: &[u8]) -> [u8; 20] {
    Ripemd160::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::hex;

    #[test]
    fn known_digests() {
        assert_eq!(
            hex::encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_concat([b"a".as_slice(), b"bc".as_slice()]),
            sha256(b"abc")
        );
        assert_eq!(
            hex::encode(&hash256(b"")),
            "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456"
        );
        assert_eq!(
            hex::encode(&ripemd160(b"abc")),
            "8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
        );
    }
}
