//! The relay's round and slot answers for its forger, answered with the internal controller over
//! a node of a corpus chain:
//!
//! - `forger.round` `{session, now, timestamps?}` -> `{round, slotNumbers}`: getCurrentRound's
//!   answer at `now`, and getSlotNumber for the given timestamps (see
//!   `tools/oracle/lib/ops/relay.js` for the input).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the relay's round answers.
pub(super) const OPS: &[(&str, Run)] = &[("forger.round", round)];

fn round(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
