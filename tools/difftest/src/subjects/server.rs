//! The server's plugin chain and inbound limits, answered with `heartwood_p2p::server`:
//!
//! - `p2p.serveRequest` `{route, hex, ip, remoteAccess, booted, known?, minimumVersions?,
//!   maxTransactionsPerRequest?, milestones?}` -> `{status, message, sent, terminated, banReason,
//!   connectionClose, acceptPort, versionRecorded}`: the plugin chain's verdict on a request with
//!   the handler stubbed (see `tools/oracle/lib/ops/server.js` for the input);
//! - `ratelimit.run` with `limiter.inbound`: the inbound limiter's answers to a sequence of
//!   requests, the forger's addresses exempt ([`inbound_rate_limit`], which the rate limiter's
//!   subject hands such a case).
//!
//! Both run the server's code over its fakes (`heartwood_p2p::testing::server`), the same
//! functions the crate's own V31 test calls.

use heartwood_p2p::testing::server;

use super::{Env, Outcome, Run, failed, height};
use crate::subject::{Case, Verdict};

/// The operations of the server's transport.
pub(super) const OPS: &[(&str, Run)] = &[("p2p.serveRequest", serve_request)];

fn serve_request(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?.into_owned();
    let network = env.devnet()?.network.clone();
    let height = height(case)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| failed(format!("no tokio runtime: {e}")))?;
    let served = server::serve_request(case.input, height, milestones, network);
    let output = runtime.block_on(served).map_err(failed)?;
    Ok(Verdict::Output(output))
}

/// `ratelimit.run` of the inbound limiter.
pub(super) fn inbound_rate_limit(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let output = server::inbound_rate_limit(case.input).map_err(failed)?;
    Ok(Verdict::Output(output))
}
