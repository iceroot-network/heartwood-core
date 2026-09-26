//! Public keys.
//!
//! The reference's acceptance set, which libsecp256k1's key parser has: 33-byte keys with prefix
//! `02` or `03` and an x coordinate on the curve; 65-byte keys with prefix `04`, or `06` and `07`
//! when the prefix matches the parity of y. Anything else, including prefix `00` and `05`, is
//! refused. A parsed key keeps the encoding it was given in, because an address hashes those bytes.

use crate::errors::KeyError;
use crate::identities::SecretKey;
use crate::utils::hex;

/// A public key that parses with the reference's acceptance set, in its original encoding.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublicKey {
    encoding: Encoding,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Encoding {
    /// Prefix `02` or `03`, then x.
    Compressed([u8; 33]),
    /// Prefix `04`, `06` or `07`, then x and y.
    Full([u8; 65]),
}

impl PublicKey {
    /// The key in `bytes`, if the reference accepts it.
    pub fn from_bytes(bytes: &[u8]) -> Result<PublicKey, KeyError> {
        secp256k1::PublicKey::from_slice(bytes).map_err(|_| KeyError::InvalidPublicKey)?;
        let encoding = if let Ok(compressed) = <[u8; 33]>::try_from(bytes) {
            Encoding::Compressed(compressed)
        } else if let Ok(full) = <[u8; 65]>::try_from(bytes) {
            Encoding::Full(full)
        } else {
            return Err(KeyError::InvalidPublicKey);
        };
        Ok(PublicKey { encoding })
    }

    /// The key in the hex `text`, if the reference accepts it.
    pub fn from_hex(text: &str) -> Result<PublicKey, KeyError> {
        PublicKey::from_bytes(&hex::decode(text)?)
    }

    /// The compressed public key of `secret`.
    pub fn from_secret_key(secret: &SecretKey) -> PublicKey {
        let point = secp256k1::PublicKey::from_secret_key(secret.backend());
        PublicKey {
            encoding: Encoding::Compressed(point.serialize()),
        }
    }

    /// Whether the reference accepts `bytes` as a public key.
    pub fn is_valid(bytes: &[u8]) -> bool {
        PublicKey::from_bytes(bytes).is_ok()
    }

    /// The key in its original encoding (33 or 65 bytes).
    pub fn as_bytes(&self) -> &[u8] {
        match &self.encoding {
            Encoding::Compressed(bytes) => bytes,
            Encoding::Full(bytes) => bytes,
        }
    }

    /// The key in compressed form.
    pub fn to_compressed(&self) -> PublicKeyBytes {
        match &self.encoding {
            Encoding::Compressed(bytes) => PublicKeyBytes(*bytes),
            Encoding::Full(bytes) => {
                // The parser has checked that a hybrid prefix matches the parity of y, so the
                // parity comes from y for every full encoding.
                let mut compressed = [0u8; 33];
                compressed[0] = 0x02 | (bytes[64] & 1);
                compressed[1..].copy_from_slice(&bytes[1..33]);
                PublicKeyBytes(compressed)
            }
        }
    }

    /// Lowercase hex of the key in its original encoding.
    pub fn to_hex(&self) -> String {
        hex::encode(self.as_bytes())
    }
}

/// The 33 bytes of a public-key field that the reference does not check to be a curve point: a
/// block's generator key and the key of a second-signature registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKeyBytes([u8; 33]);

impl PublicKeyBytes {
    /// These 33 bytes.
    pub const fn from_array(bytes: [u8; 33]) -> PublicKeyBytes {
        PublicKeyBytes(bytes)
    }

    /// The 33 bytes in the hex `text`.
    pub fn from_hex(text: &str) -> Result<PublicKeyBytes, KeyError> {
        Ok(PublicKeyBytes(hex::decode_array(text)?))
    }

    /// The 33 bytes.
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.0
    }

    /// Lowercase hex of the 33 bytes.
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// The bytes as a [`PublicKey`], if the reference accepts them as one.
    pub fn parse(&self) -> Result<PublicKey, KeyError> {
        PublicKey::from_bytes(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: &str = "4151a3ec46b5670a682b0a63394f863587d1bc97483b1b6c70eb58e7f0aed192";
    const Y: &str = "fa29ce00fb8ed92f34f68c0018f12d5b68c1609080abaf331746d6d86c6039e5";

    #[test]
    fn acceptance_set() {
        for prefix in ["02", "03"] {
            assert!(PublicKey::is_valid(
                &hex::decode(&format!("{prefix}{X}")).unwrap()
            ));
        }
        // y is odd, so 07 is the matching hybrid prefix.
        for (prefix, valid) in [("04", true), ("06", false), ("07", true), ("05", false)] {
            let bytes = hex::decode(&format!("{prefix}{X}{Y}")).unwrap();
            assert_eq!(PublicKey::is_valid(&bytes), valid, "prefix {prefix}");
        }
        for prefix in ["00", "01", "05", "ff"] {
            assert!(!PublicKey::is_valid(
                &hex::decode(&format!("{prefix}{X}")).unwrap()
            ));
        }
        assert!(!PublicKey::is_valid(&[]));
        assert!(!PublicKey::is_valid(&hex::decode(X).unwrap()));
    }

    #[test]
    fn original_encoding_and_compressed_form() {
        let full = PublicKey::from_hex(&format!("07{X}{Y}")).unwrap();
        assert_eq!(full.to_hex(), format!("07{X}{Y}"));
        assert_eq!(full.as_bytes().len(), 65);
        assert_eq!(full.to_compressed().to_hex(), format!("03{X}"));
        let uncompressed = PublicKey::from_hex(&format!("04{X}{Y}")).unwrap();
        assert_eq!(uncompressed.to_compressed(), full.to_compressed());
        assert_ne!(uncompressed, full);
        let compressed = full.to_compressed().parse().unwrap();
        assert_eq!(compressed.to_compressed(), full.to_compressed());
    }

    #[test]
    fn unchecked_bytes() {
        let bytes = PublicKeyBytes::from_hex(&format!("00{X}")).unwrap();
        assert_eq!(bytes.to_hex(), format!("00{X}"));
        assert_eq!(bytes.parse(), Err(KeyError::InvalidPublicKey));
    }
}
