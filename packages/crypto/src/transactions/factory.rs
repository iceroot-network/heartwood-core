//! Making a [`Transaction`] from bytes, fields or JSON.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::{BuildError, DecodeError};
use crate::managers::Params;
use crate::transactions::types::{Transaction, TransactionData};

impl Transaction {
    /// Decode `bytes` under the merged milestone `params` of the rule height, as the reference's
    /// strict `TransactionFactory.fromBytes` does: deserialise, compute the id, apply the
    /// transaction rules, and check the sender's signature.
    ///
    /// A bad signature does not refuse the transaction: it decodes with
    /// [`Transaction::is_verified`] false, and the block or pool rules decide.
    pub fn decode(bytes: &[u8], params: &Params) -> Result<Transaction, DecodeError> {
        todo!()
    }

    /// Decode `bytes` that this node verified before, as the reference's
    /// `TransactionFactory.fromBytesUnsafe` does: no transaction rules and no signature check, and
    /// the transaction counts as verified.
    ///
    /// Callers, each of which must only pass bytes verified before: [`crate::Block::decode_trusted`].
    pub fn decode_trusted(bytes: &[u8], params: &Params) -> Result<Transaction, DecodeError> {
        todo!()
    }

    /// The transaction with fields `data`, as the reference's `TransactionFactory.fromData` makes
    /// it: apply the transaction rules, serialise, then [`Transaction::decode`] the bytes.
    pub fn from_data(data: TransactionData, params: &Params) -> Result<Transaction, BuildError> {
        todo!()
    }

    /// The transaction in the reference's JSON form, as `TransactionFactory.fromJson` reads it.
    pub fn from_json(json: &serde_json::Value, params: &Params) -> Result<Transaction, BuildError> {
        todo!()
    }
}
