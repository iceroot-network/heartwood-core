//! Public keys.
//!
//! The reference's acceptance set, which libsecp256k1's key parser has: 33-byte keys with prefix
//! `02` or `03` and an x coordinate on the curve; 65-byte keys with prefix `04`, or `06` and `07`
//! when the prefix matches the parity of y. Anything else, including prefix `00` and `05`, is
//! refused. A parsed key keeps the encoding it was given in, because an address hashes those bytes.

#![expect(
    unused_variables,
    dead_code,
    reason = "the bodies are not written yet; remove this once they are"
)]

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
    Compressed([u8; 33]),
    Full([u8; 65]),
}

impl PublicKey {
    /// The key in `bytes`, if the reference accepts it.
    pub fn from_bytes(bytes: &[u8]) -> Result<PublicKey, KeyError> {
        todo!()
    }

    /// The key in the hex `text`, if the reference accepts it.
    pub fn from_hex(text: &str) -> Result<PublicKey, KeyError> {
        PublicKey::from_bytes(&hex::decode(text)?)
    }

    /// The compressed public key of `secret`.
    pub fn from_secret_key(secret: &SecretKey) -> PublicKey {
        todo!()
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
        todo!()
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
