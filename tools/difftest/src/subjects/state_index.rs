//! `state.indexOrders`: the address, public-key and username index orders of the state after a
//! chain's blocks, after a restart from the saved state, and after a restart that replays the chain,
//! against the reference's live, saved-state and StateBuilder orders.

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("state.indexOrders", index_orders)];

fn index_orders(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("state.indexOrders has no Rust implementation yet"))
}
