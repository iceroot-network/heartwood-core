//! Signing transaction fields.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::{Aux, Signature};
use crate::errors::BuildError;
use crate::identities::KeyPair;
use crate::managers::Params;
use crate::transactions::types::TransactionData;

/// Sign the unsigned bytes of `data` with `keys` (BIP340, `aux`) and store the signature in
/// `data.signature`.
pub fn sign(
    data: &mut TransactionData,
    keys: &KeyPair,
    aux: Aux,
    params: &Params,
) -> Result<Signature, BuildError> {
    todo!()
}

/// Sign the bytes of `data` without its second signature with the second key `keys` (BIP340,
/// `aux`) and store the signature in `data.second_signature`.
pub fn second_sign(
    data: &mut TransactionData,
    keys: &KeyPair,
    aux: Aux,
    params: &Params,
) -> Result<Signature, BuildError> {
    todo!()
}
