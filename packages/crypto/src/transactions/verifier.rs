//! Signature checks and the transaction rules.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::SchemaError;
use crate::managers::Params;
use crate::transactions::types::{Transaction, TransactionData};

/// Whether the sender's signature of `data` is valid (BIP340 over the unsigned bytes).
pub fn verify(data: &TransactionData, params: &Params) -> bool {
    todo!()
}

/// Whether the second signature of `data` is valid for the second key `public_key`, given as the
/// reference stores it (see [`crate::crypto::sig::verify`] for how the key bytes are read).
pub fn verify_second_signature(data: &TransactionData, public_key: &[u8], params: &Params) -> bool {
    todo!()
}

/// Apply the transaction rules to `data` (see [`crate::validation`]). `strict` refuses fields that
/// the type does not have, as the reference's strict schemas do.
pub fn verify_schema(
    data: &TransactionData,
    params: &Params,
    strict: bool,
) -> Result<(), SchemaError> {
    todo!()
}

impl Transaction {
    /// Whether the second signature is valid for the second key `public_key`.
    pub fn verify_second_signature(&self, public_key: &[u8], params: &Params) -> bool {
        verify_second_signature(&self.data, public_key, params)
    }
}
