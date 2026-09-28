//! The server's plugin chain and inbound limits, answered with `heartwood_p2p::server`:
//!
//! - `p2p.serveRequest` `{route, hex, ip, remoteAccess, booted, minimumVersions?, milestones?}`
//!   -> `{status, message, banReason, connectionClose}`: the plugin chain's verdict on a request
//!   with the handler stubbed (see `tools/oracle/lib/ops/server.js` for the input);
//! - `ratelimit.run` with `limiter.inbound`: the inbound limiter's answers to a sequence of
//!   requests, the forger's addresses exempt ([`inbound_rate_limit`], which the rate limiter's
//!   subject hands such a case).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the server's transport.
pub(super) const OPS: &[(&str, Run)] = &[("p2p.serveRequest", serve_request)];

fn serve_request(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

/// `ratelimit.run` of the inbound limiter.
pub(super) fn inbound_rate_limit(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
