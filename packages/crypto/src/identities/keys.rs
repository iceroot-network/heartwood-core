//! Secret keys and key pairs.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::errors::KeyError;
use crate::identities::PublicKey;

/// A secp256k1 secret key. Its bytes are wiped when it is dropped, and `Debug` does not show them.
pub struct SecretKey {
    bytes: [u8; 32],
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl ZeroizeOnDrop for SecretKey {}

impl SecretKey {
    /// The secret key with these bytes, which must be a nonzero number below the curve order.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<SecretKey, KeyError> {
        todo!()
    }

    /// The secret key in the hex `text` (64 hex digits).
    pub fn from_hex(text: &str) -> Result<SecretKey, KeyError> {
        todo!()
    }

    /// The passphrase key: the SHA-256 of the passphrase's UTF-8 bytes.
    pub fn from_passphrase(passphrase: &str) -> Result<SecretKey, KeyError> {
        todo!()
    }

    /// Lowercase hex of the key, for key files. The text is wiped when it is dropped.
    pub fn to_hex(&self) -> Zeroizing<String> {
        todo!()
    }

    /// The key bytes, for the signature backends.
    pub(crate) fn expose(&self) -> &[u8; 32] {
        &self.bytes
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
