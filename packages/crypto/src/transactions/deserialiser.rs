//! Bytes to transaction fields.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::DecodeError;
use crate::transactions::types::TransactionData;

/// The fields in `bytes`: header, memo, asset and signatures, with every byte consumed. Removed
/// and unknown types are refused here.
pub fn deserialise(bytes: &[u8]) -> Result<TransactionData, DecodeError> {
    todo!()
}
