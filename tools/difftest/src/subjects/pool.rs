//! The transaction pool: a pool opened over a wallet fixture or a chain, batches of entries
//! processed at a given time, the online record, re-adds, clean-ups, removals, flushes, blocks
//! applied with the synced node's sequence, the pool's contents in the reference's orders,
//! collations and a restart of the pool's storage.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("pool.open", not_yet),
    ("pool.process", not_yet),
    ("pool.setOnline", not_yet),
    ("pool.readd", not_yet),
    ("pool.cleanUp", not_yet),
    ("pool.remove", not_yet),
    ("pool.removeForged", not_yet),
    ("pool.flush", not_yet),
    ("pool.blockApplied", not_yet),
    ("pool.contents", not_yet),
    ("pool.collate", not_yet),
    ("pool.restart", not_yet),
];

/// An operation without a Rust subject yet: its cases are skipped.
fn not_yet(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(format!(
        "{} has no subject yet",
        case.op
    )))
}
