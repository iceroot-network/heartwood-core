//! The forger's decisions, answered with `heartwood_forger::forger_service` over a scripted relay
//! and a recording signer:
//!
//! - `forger.decide` `{round, networkState, status, slots, timeLeftInMs, delegate, firstAttempt,
//!   usernames, overHeightHeaders, milestones}` -> `{action, reason, lines}`: the branches of the
//!   reference's `forgeNewBlock` and `isForgingAllowed` (see `tools/oracle/lib/ops/forger-decide.js`
//!   for the input), with `Documented::OverHeightOwnBlock` for an over-height header by the
//!   delegate's own key that the reference cannot verify.
//!
//! The internal routes' bodies of the same class (the reference's codecs over its own
//! `NetworkState` and `getCurrentRound` objects) are answered with `heartwood_p2p_wire::internal`,
//! through operations this table adds.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the forger's decisions and the internal bodies.
pub(super) const OPS: &[(&str, Run)] = &[("forger.decide", decide)];

fn decide(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
