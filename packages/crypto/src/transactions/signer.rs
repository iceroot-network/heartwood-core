//! Signing transaction fields.

use crate::crypto::sig::{self, Aux, SchemeId, Signature, SigningDomain};
use crate::errors::BuildError;
use crate::identities::KeyPair;
use crate::managers::Params;
use crate::transactions::serialiser::{SerialiseOptions, signing_hash};
use crate::transactions::types::TransactionData;

/// Sign the unsigned bytes of `data` with `keys` (BIP340, `aux`) and store the signature in
/// `data.signature`.
///
/// No rule is checked: [`crate::Transaction::from_data`] does that. The fields are signed as they
/// are, so a caller can sign data the rules would refuse, as the reference's signer can.
pub fn sign(
    data: &mut TransactionData,
    keys: &KeyPair,
    aux: Aux,
    params: &Params,
) -> Result<Signature, BuildError> {
    let hash = signing_hash(data, SerialiseOptions::UNSIGNED, params)?;
    let signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &hash,
        keys.secret_key(),
        aux,
    )?;
    data.signature = Some(signature);
    Ok(signature)
}

/// Sign the bytes of `data` without its second signature with the second key `keys` (BIP340,
/// `aux`) and store the signature in `data.second_signature`. The sender's signature must be in
/// `data` already, because the second key signs it too.
pub fn second_sign(
    data: &mut TransactionData,
    keys: &KeyPair,
    aux: Aux,
    params: &Params,
) -> Result<Signature, BuildError> {
    let hash = signing_hash(data, SerialiseOptions::FOR_SECOND_SIGNATURE, params)?;
    let signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &hash,
        keys.secret_key(),
        aux,
    )?;
    data.second_signature = Some(signature);
    Ok(signature)
}
