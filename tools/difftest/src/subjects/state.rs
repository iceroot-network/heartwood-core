//! Wallets, ranking and rounds: the forging shuffle, the delegate ranking and a voter's shares.

use super::{Env, Outcome};
use crate::subject::{Case, SubjectError, Verdict};

/// `state.shuffle`: the forging order of a round.
pub(super) fn shuffle(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `state.rank`: the ranks and the round's delegates of a set of delegates.
pub(super) fn rank(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `wallet.voteAmounts`: each delegate's share of a voter's balance.
pub(super) fn vote_amounts(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}
