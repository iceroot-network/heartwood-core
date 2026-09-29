//! `chain.rollback`: `snapshot rollback` of a chain session's store, its refusal or its target,
//! lines, deleted rows and store summary, as the oracle runs the reference's snapshot service.

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("chain.rollback", rollback)];

fn rollback(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("chain.rollback has no Rust implementation yet"))
}
