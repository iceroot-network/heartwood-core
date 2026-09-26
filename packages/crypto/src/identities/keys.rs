//! Secret keys and key pairs.

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use crate::crypto::hash::sha256;
use crate::errors::{HexError, KeyError};
use crate::identities::PublicKey;
use crate::utils::hex;

/// A secp256k1 secret key: a nonzero number below the curve order.
///
/// Its bytes are overwritten when it is dropped, `Debug` does not show them, and it is not `Clone`.
pub struct SecretKey {
    inner: secp256k1::SecretKey,
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.inner.non_secure_erase();
    }
}

impl SecretKey {
    /// The secret key with these bytes, which must be a nonzero number below the curve order.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<SecretKey, KeyError> {
        let mut bytes = bytes;
        let parsed = secp256k1::SecretKey::from_secret_bytes(bytes);
        bytes.zeroize();
        parsed
            .map(|inner| SecretKey { inner })
            .map_err(|_| KeyError::InvalidSecretKey)
    }

    /// The secret key in the hex `text` (64 hex digits).
    pub fn from_hex(text: &str) -> Result<SecretKey, KeyError> {
        let decoded = Zeroizing::new(hex::decode(text)?);
        let mut bytes = Zeroizing::new([0u8; 32]);
        if decoded.len() != bytes.len() {
            return Err(KeyError::Hex(HexError::WrongLength {
                expected: bytes.len(),
                actual: decoded.len(),
            }));
        }
        bytes.copy_from_slice(&decoded);
        SecretKey::from_bytes(*bytes)
    }

    /// The passphrase key: the SHA-256 of the passphrase's UTF-8 bytes.
    ///
    /// The digest is refused only if it is zero or not below the curve order, which no known
    /// passphrase gives.
    pub fn from_passphrase(passphrase: &str) -> Result<SecretKey, KeyError> {
        let digest = Zeroizing::new(sha256(passphrase.as_bytes()));
        SecretKey::from_bytes(*digest)
    }

    /// Lowercase hex of the key, for key files. The text is wiped when it is dropped.
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(hex::encode(self.expose()))
    }

    /// The key bytes, for the signature backends.
    pub(crate) fn expose(&self) -> &[u8; 32] {
        self.inner.as_secret_bytes()
    }

    /// The key as the curve library holds it, for the signature backends.
    pub(crate) fn backend(&self) -> &secp256k1::SecretKey {
        &self.inner
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

/// A secret key and its compressed public key.
#[derive(Debug)]
pub struct KeyPair {
    secret: SecretKey,
    public: PublicKey,
}

impl KeyPair {
    /// The key pair of a passphrase (see [`SecretKey::from_passphrase`]).
    pub fn from_passphrase(passphrase: &str) -> Result<KeyPair, KeyError> {
        SecretKey::from_passphrase(passphrase).map(KeyPair::from_secret_key)
    }

    /// The key pair of `secret`, with its public key in compressed form.
    pub fn from_secret_key(secret: SecretKey) -> KeyPair {
        let public = PublicKey::from_secret_key(&secret);
        KeyPair { secret, public }
    }

    /// The secret key.
    pub fn secret_key(&self) -> &SecretKey {
        &self.secret
    }

    /// The compressed public key.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: &str = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141";

    #[test]
    fn range() {
        assert_eq!(
            SecretKey::from_bytes([0; 32]).unwrap_err(),
            KeyError::InvalidSecretKey
        );
        assert!(SecretKey::from_hex(ORDER).is_err());
        let below = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140";
        assert_eq!(SecretKey::from_hex(below).unwrap().to_hex().as_str(), below);
        let one = SecretKey::from_hex(&format!("{:064x}", 1)).unwrap();
        assert_eq!(one.expose()[31], 1);
    }

    #[test]
    fn hex_errors() {
        assert_eq!(
            SecretKey::from_hex("00").unwrap_err(),
            KeyError::Hex(HexError::WrongLength {
                expected: 32,
                actual: 1
            })
        );
        assert_eq!(
            SecretKey::from_hex("0").unwrap_err(),
            KeyError::Hex(HexError::OddLength)
        );
    }

    #[test]
    fn passphrase() {
        let pair = KeyPair::from_passphrase("this is a top secret passphrase").unwrap();
        assert_eq!(
            pair.secret_key().to_hex().as_str(),
            "d8839c2432bfd0a67ef10a804ba991eabba19f154a3d707917681d45822a5712"
        );
        assert_eq!(
            pair.public_key().to_hex(),
            "034151a3ec46b5670a682b0a63394f863587d1bc97483b1b6c70eb58e7f0aed192"
        );
        assert_eq!(format!("{:?}", pair.secret_key()), "SecretKey(..)");
    }
}
