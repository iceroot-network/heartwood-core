//! Signature checks and the transaction rules.

use crate::crypto::sig::{self, SchemeId, SigningDomain};
use crate::errors::SchemaError;
use crate::managers::Params;
use crate::transactions::serialiser::{SerialiseOptions, signing_hash};
use crate::transactions::types::{Transaction, TransactionData};
use crate::validation::{Mode, validate_transaction};

/// Whether the sender's signature of `data` is valid (BIP340 over the unsigned bytes).
pub fn verify(data: &TransactionData, params: &Params) -> bool {
    let Some(signature) = &data.signature else {
        return false;
    };
    let Ok(hash) = signing_hash(data, SerialiseOptions::UNSIGNED, params) else {
        return false;
    };
    sig::verify(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &hash,
        signature,
        data.sender_public_key.as_bytes(),
    )
}

/// Whether the second signature of `data` is valid for the second key `public_key`, given as the
/// reference stores it (see [`crate::crypto::sig::verify`] for how the key bytes are read).
pub fn verify_second_signature(data: &TransactionData, public_key: &[u8], params: &Params) -> bool {
    let Some(signature) = &data.second_signature else {
        return false;
    };
    let Ok(hash) = signing_hash(data, SerialiseOptions::FOR_SECOND_SIGNATURE, params) else {
        return false;
    };
    sig::verify(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &hash,
        signature,
        public_key,
    )
}

/// Whether `data` passes the transaction rules (see [`crate::validation`]) under `mode`. Vote
/// names that are not delegate names do not refuse it: the rules remove them, which
/// [`validate_transaction`] shows.
pub fn verify_schema(
    data: &TransactionData,
    params: &Params,
    mode: Mode,
) -> Result<(), SchemaError> {
    validate_transaction(&mut data.clone(), params, mode)
}

impl Transaction {
    /// Whether the second signature is valid for the second key `public_key`.
    pub fn verify_second_signature(&self, public_key: &[u8], params: &Params) -> bool {
        verify_second_signature(&self.data, public_key, params)
    }
}
