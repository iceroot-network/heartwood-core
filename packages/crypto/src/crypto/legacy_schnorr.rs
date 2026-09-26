//! The reference's pre-BIP340 Schnorr signatures over secp256k1.
//!
//! Built on the public API of libsecp256k1 (scalar and point tweaks, key combination) and the
//! Jacobi-symbol helper in `legendre`, so secret keys never enter a second curve library. The
//! nonce is derived from the key and the message, as the reference does. A nonce point at infinity
//! is refused. Used for peer status attestations only.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::Signature;
use crate::errors::SigError;
use crate::identities::SecretKey;

/// The legacy Schnorr signature of `message` by `secret`.
pub(super) fn sign(message: &[u8], secret: &SecretKey) -> Result<Signature, SigError> {
    todo!()
}

/// Whether `signature` is a valid legacy Schnorr signature of `message` for the key bytes
/// `public_key`, which must parse with the reference's acceptance set.
pub(super) fn verify(message: &[u8], signature: &Signature, public_key: &[u8]) -> bool {
    todo!()
}
