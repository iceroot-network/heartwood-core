//! Attestations: signing the preimage, assembling the response, and checking one signature.

use heartwood_crypto::errors::SigError;
use heartwood_crypto::{KeyPair, PublicKeyBytes, Signature};

use super::tree::{PeerConfig, SignedState};
use crate::messages::peer::GetStatusResponse;

/// One attestation of a status: a delegate's key and its signature of the preimage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// The delegate's public key.
    pub public_key: PublicKeyBytes,
    /// The legacy Schnorr signature of the preimage.
    pub signature: Signature,
}

/// Sign `preimage` with `key`: legacy Schnorr over the raw bytes, with no hash and no aux.
pub fn attest(preimage: &[u8], key: &KeyPair) -> Result<Attestation, SigError> {
    let _ = (preimage, key);
    todo!()
}

/// The getStatus response: the state and configuration as the wire carries them (the slot as its
/// unsigned 32-bit value, the amounts as decimal text, the plugins in their object order), and
/// the attestations' keys and signatures in hex, in the given order.
pub fn response(
    state: &SignedState,
    config: &PeerConfig,
    attestations: &[Attestation],
) -> GetStatusResponse {
    let _ = (state, config, attestations);
    todo!()
}

/// Whether `signature_hex` is a legacy Schnorr signature of `preimage` by `public_key_hex`, as the
/// reference's `verifySchnorr` decides for a received pair. Text that is not a key or a
/// signature fails the check.
pub fn verify_attestation(preimage: &[u8], public_key_hex: &str, signature_hex: &str) -> bool {
    let _ = (preimage, public_key_hex, signature_hex);
    todo!()
}
