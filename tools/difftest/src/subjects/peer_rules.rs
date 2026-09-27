//! The peer rules, answered with `heartwood_p2p::utils` and `heartwood_p2p::rate_limiter`:
//!
//! - `peer.isValid` `{ip, allowPrivate, interfaces}` -> `{valid, processed}`;
//! - `peer.isValidVersion` `{version, ranges, networkName}` -> `{valid}`;
//! - `peer.sameSubnet` `{ip, peers}` -> `{same}`;
//! - `ratelimit.run` `{limiter, events: [{t, ip, route, op}]}` -> the answer to each event.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the peer rules.
pub(super) const OPS: &[(&str, Run)] = &[
    ("peer.isValid", is_valid),
    ("peer.isValidVersion", is_valid_version),
    ("peer.sameSubnet", same_subnet),
    ("ratelimit.run", rate_limit),
];

fn is_valid(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn is_valid_version(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn same_subnet(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn rate_limit(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
