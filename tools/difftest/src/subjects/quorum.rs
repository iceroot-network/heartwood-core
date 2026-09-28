//! The network state a relay gives its forger, answered with `heartwood_p2p::network_state`:
//!
//! - `quorum.analyze` `{peers, declared, forgerKeys, active, ranks, tip, now, coldStart,
//!   minimumDelegateReach?}` -> the network state's JSON and `canForge` (see
//!   `tools/oracle/lib/ops/relay.js` for the input).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the network state.
pub(super) const OPS: &[(&str, Run)] = &[("quorum.analyze", analyze)];

fn analyze(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
