//! BIP340 Schnorr signatures over secp256k1, through libsecp256k1.
//!
//! The message rule is the reference's: 32 bytes are signed as given, any other length is hashed
//! with SHA-256 first. Verification reads a 33-byte key as its x coordinate (bytes 1 to 32) without
//! checking the prefix byte.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::Signature;
use crate::identities::SecretKey;

/// The BIP340 signature of `message` by `secret`, with the aux bytes `aux`.
pub(super) fn sign(message: &[u8], secret: &SecretKey, aux: &[u8; 32]) -> Signature {
    todo!()
}

/// Whether `signature` is a valid BIP340 signature of `message` for the key bytes `public_key`.
pub(super) fn verify(message: &[u8], signature: &Signature, public_key: &[u8]) -> bool {
    todo!()
}
