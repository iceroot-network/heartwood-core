//! The transaction handlers: the dynamic fee floor, and one transaction applied to a wallet
//! fixture and reverted.

use heartwood_crypto::Transaction;
use heartwood_transactions::minimum_fee;
use serde_json::json;

use super::{Env, Outcome, decode_refused, hex_bytes, params, rejected};
use crate::subject::{Case, SubjectError, Verdict};

/// `fee.minimum`: the dynamic fee floor of a transaction decoded (strictly) at the case's height,
/// with its type key and byte length.
pub(super) fn fee_minimum(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    Ok(match Transaction::decode(&bytes, params) {
        Ok(tx) => Verdict::Output(json!({
            "key": tx.key(),
            "size": tx.bytes().len(),
            "minimumFee": minimum_fee(&tx, params).to_string(),
        })),
        Err(error) => decode_refused(&error, &bytes),
    })
}

/// `state.applyTransaction`: a transaction applied to a fixture, then reverted.
pub(super) fn apply_transaction(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}
