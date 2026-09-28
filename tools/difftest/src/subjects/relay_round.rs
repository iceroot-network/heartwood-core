//! The relay's round and slot answers for its forger, answered with the internal controller over
//! a node of a corpus chain:
//!
//! - `forger.round` `{chain: {crypto, blocks}, at: [{now, timestamps?}]}` -> `{answers: [{round,
//!   slotNumbers}]}`: a node started on the chain definition `crypto` with the blocks of `blocks`
//!   applied, then at each moment getCurrentRound's answer (its text) and getSlotNumber for each
//!   of the timestamps (see `tools/oracle/lib/ops/relay.js` for the input).
//!
//! Heartwood answers both routes through the node's services (`NodeServices::current_round` and
//! `NodeServices::slot_number`), which read a running node's published state, its round rows and
//! its pool. The subject needs such a node over the case's chain, which the relay's composition
//! root builds; until it does, the records report that they have no Rust implementation.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the relay's round answers.
pub(super) const OPS: &[(&str, Run)] = &[("forger.round", round)];

fn round(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
