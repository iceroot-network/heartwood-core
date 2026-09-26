//! Transactions to bytes, and the hashes that signatures and ids are computed over.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::EncodeError;
use crate::managers::Params;
use crate::transactions::types::{TransactionData, TransactionId};

/// Which signatures a serialisation leaves out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SerialiseOptions {
    /// Leave out the sender's signature.
    pub exclude_signature: bool,
    /// Leave out the second signature.
    pub exclude_second_signature: bool,
}

impl SerialiseOptions {
    /// Every signature: the bytes an id is computed over.
    pub const FULL: SerialiseOptions = SerialiseOptions {
        exclude_signature: false,
        exclude_second_signature: false,
    };

    /// No signature: the bytes the sender signs.
    pub const UNSIGNED: SerialiseOptions = SerialiseOptions {
        exclude_signature: true,
        exclude_second_signature: true,
    };

    /// The sender's signature only: the bytes the second key signs.
    pub const FOR_SECOND_SIGNATURE: SerialiseOptions = SerialiseOptions {
        exclude_signature: false,
        exclude_second_signature: true,
    };
}

/// The serialiser's size limit at the height of `params`: `floor(maxPayload / maxTransactions) × 2`
/// bytes, or 83,886 when either value is missing.
pub fn size_limit(params: &Params) -> usize {
    todo!()
}

/// The bytes of `data` with the signatures that `options` keeps.
pub fn serialise(
    data: &TransactionData,
    options: SerialiseOptions,
    params: &Params,
) -> Result<Vec<u8>, EncodeError> {
    todo!()
}

/// The SHA-256 of [`serialise`]: the hash that a signature signs.
pub fn signing_hash(
    data: &TransactionData,
    options: SerialiseOptions,
    params: &Params,
) -> Result<[u8; 32], EncodeError> {
    todo!()
}

/// The id of `data`: the SHA-256 of its full serialisation. A transfer to an address of another
/// network has no id, as in the reference.
pub fn transaction_id(
    data: &TransactionData,
    params: &Params,
) -> Result<TransactionId, EncodeError> {
    todo!()
}
