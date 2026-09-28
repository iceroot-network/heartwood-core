//! The served status, answered with `heartwood_p2p::status_server`:
//!
//! - `status.serve` `{tip, now, peerConfig, registered, forgerKeys}` -> `{hex, preimage}`: the
//!   getStatus answer's bytes and its signing preimage (see `tools/oracle/lib/ops/relay.js` for
//!   the input).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the served status.
pub(super) const OPS: &[(&str, Run)] = &[("status.serve", serve)];

fn serve(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
