//! The transaction handlers: the dynamic fee floor, and one transaction applied to a wallet
//! fixture and reverted.

use super::{Env, Outcome};
use crate::subject::{Case, SubjectError, Verdict};

/// `fee.minimum`: the dynamic fee floor of a transaction.
pub(super) fn fee_minimum(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `state.applyTransaction`: a transaction applied to a fixture, then reverted.
pub(super) fn apply_transaction(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}
