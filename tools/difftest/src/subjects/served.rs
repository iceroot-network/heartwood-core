//! `p2p.controller`: a public route's controller of the server over a chain, a pool and a peer
//! repository, its answer encoded with the route's codec and its log lines, as the oracle runs the
//! reference's controller headless.

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("p2p.controller", controller)];

fn controller(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("p2p.controller has no Rust implementation yet"))
}
