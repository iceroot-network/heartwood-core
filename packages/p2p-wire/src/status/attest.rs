//! Attestations: signing the preimage, assembling the response, and checking one signature.

use heartwood_crypto::crypto::sig;
use heartwood_crypto::errors::SigError;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, KeyPair, PublicKeyBytes, SchemeId, Signature, SigningDomain};

use super::tree::{PeerConfig, SignedState, config_message, state_message};
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
    // The legacy scheme derives its nonce from the key and the message and reads no aux.
    let signature = sig::sign(
        SchemeId::Secp256k1Legacy,
        SigningDomain::PeerStatus,
        preimage,
        key.secret_key(),
        Aux::random(),
    )?;
    Ok(Attestation {
        public_key: key.public_key().to_compressed(),
        signature,
    })
}

/// The getStatus response: the state and configuration as the wire carries them (the slot as its
/// unsigned 32-bit value, the amounts as decimal text, the plugins in their object order), and
/// the attestations' keys and signatures in hex, in the given order.
pub fn response(
    state: &SignedState,
    config: &PeerConfig,
    attestations: &[Attestation],
) -> GetStatusResponse {
    GetStatusResponse {
        state: Some(state_message(state)),
        config: Some(config_message(config)),
        public_keys: attestations.iter().map(|a| a.public_key.to_hex()).collect(),
        signatures: attestations.iter().map(|a| a.signature.to_hex()).collect(),
        ..GetStatusResponse::default()
    }
}

/// Whether `signature_hex` is a legacy Schnorr signature of `preimage` by `public_key_hex`, as the
/// reference's `verifySchnorr` decides for a received pair. Text that is not a key or a
/// signature fails the check.
pub fn verify_attestation(preimage: &[u8], public_key_hex: &str, signature_hex: &str) -> bool {
    let (Ok(key), Ok(signature)) = (
        hex::decode(public_key_hex),
        Signature::from_hex(signature_hex),
    ) else {
        return false;
    };
    sig::verify(
        SchemeId::Secp256k1Legacy,
        SigningDomain::PeerStatus,
        preimage,
        &signature,
        &key,
    )
}
