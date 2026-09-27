//! `chain.process`: the block processor on a session's chain, answered with
//! `heartwood_blockchain::processor`: `{session, hex, now}` -> `{result, unchained?, line?}`, the
//! processor's result, the unchained status when the block did not chain, and the state digest
//! line after an accepted block.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the processor.
pub(super) const OPS: &[(&str, Run)] = &[("chain.process", process)];

fn process(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
