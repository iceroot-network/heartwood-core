//! The transaction rules that the reference writes as JSON schemas, written by hand.
//!
//! Each rule has the reference's observable semantics: the same values accepted and refused, in the
//! same order where the first failure is observable. Vote rules: the shares add up to exactly
//! 10,000 basis points unless there are none; from JSON, a share must be a multiple of 0.01 before
//! it becomes `round(value × 100)` basis points.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::SchemaError;
use crate::managers::Params;
use crate::transactions::TransactionData;
use crate::transactions::types::Votes;

/// Apply the rules of the transaction's type to `data` under the merged milestone `params`.
/// `strict` also refuses fields the type does not have.
pub fn validate_transaction(
    data: &TransactionData,
    params: &Params,
    strict: bool,
) -> Result<(), SchemaError> {
    todo!()
}

/// Whether `username` is a valid delegate name.
pub fn validate_username(username: &str) -> Result<(), SchemaError> {
    todo!()
}

/// Whether `votes` is a valid vote asset: shares that add up to exactly 10,000 basis points, or no
/// votes at all, and names that are valid delegate names.
pub fn validate_votes(votes: &Votes) -> Result<(), SchemaError> {
    todo!()
}

/// The basis points of the vote percentage `percent` read from JSON: refused unless it is a
/// multiple of 0.01 from 0 to 100, then `round(percent × 100)`.
pub fn vote_basis_points(percent: f64) -> Result<u16, SchemaError> {
    todo!()
}
