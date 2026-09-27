//! The network monitor's decisions and the download path, answered with `heartwood_p2p`:
//!
//! - `monitor.networkHeight` `{heights}` -> `{height}`;
//! - `monitor.forkDecision` `{peers, height}` -> `{forked, blocksToRollback}`;
//! - `monitor.downloadPlan` `{peers, networkHeight, from, chunkSize, maxParallel, answers, order}`
//!   -> `{requests, blocks, cached, nextChunkSize}`, with the reference's recorded order of the
//!   backup peers injected;
//! - `block.fromListed` `{hex}` -> `{blocks: [{id, verdict}]}`: the codec, the reply check, the
//!   transactions read as `fromBytesUnsafe` reads them, and `heartwood_p2p::to_block`.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the monitor.
pub(super) const OPS: &[(&str, Run)] = &[
    ("monitor.networkHeight", network_height),
    ("monitor.forkDecision", fork_decision),
    ("monitor.downloadPlan", download_plan),
    ("block.fromListed", from_listed),
];

fn network_height(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn fork_decision(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn download_plan(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn from_listed(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
