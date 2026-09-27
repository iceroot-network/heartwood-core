//! The peer verifier, answered with `heartwood_p2p::nsect` and `heartwood_p2p::verifier`:
//!
//! - `nsect.find` `{n, low, high, common}` -> `{probes, result}`;
//! - `verifier.checkState` `{session, peer, claimed, fast}` -> `{result, probes, fetched}`, with
//!   `Documented::UnlinkedForkSegment` for a segment whose headers do not chain.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the verifier.
pub(super) const OPS: &[(&str, Run)] = &[
    ("nsect.find", nsect_find),
    ("verifier.checkState", check_state),
];

fn nsect_find(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn check_state(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
