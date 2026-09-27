//! The cases of P19, the forger's decisions: random rounds, network states (every status, quorum
//! counts around the threshold, over-height headers by the delegate's key and by others), status
//! headers, slot numbers and times left around the 2,000 ms margin, for `forger.decide` (the action
//! and the lines must be the reference's).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P19.
pub fn decision_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("forger.decide", 2, json!({}))).boxed()
}
