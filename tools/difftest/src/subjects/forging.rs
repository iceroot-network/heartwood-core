//! Block assembly and signing, answered with `heartwood_forger::delegate`:
//!
//! - `forger.forge` `{privateKey, aux, previousBlock: {id, height}, timestamp, reward,
//!   transactions, milestones?}` -> `{hex, id}`: each transaction read as `fromBytesUnsafe` reads
//!   it, then `Delegate.forge`;
//! - `forger.collateAndForge` `{session, privateKey, aux, round}` -> `{collated, removed, hex, id}`:
//!   the pool session's collation for a forger (`validate` on, nothing excluded), then
//!   `Delegate.forge` (see `tools/oracle/lib/ops/forger-forge.js` for the input).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of block assembly.
pub(super) const OPS: &[(&str, Run)] = &[
    ("forger.forge", forge),
    ("forger.collateAndForge", collate_and_forge),
];

fn forge(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn collate_and_forge(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
