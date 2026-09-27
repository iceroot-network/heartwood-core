//! `peer.attest`: the attestation loop of a ping, answered with
//! `heartwood_p2p::attestation::attest`. The input is a checked status, the other peers' keys,
//! forks and headers, our last block, the current slot and the delegates; the output is
//! `{attributed, versions, moved}`.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the peers.
pub(super) const OPS: &[(&str, Run)] = &[("peer.attest", attest)];

fn attest(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
