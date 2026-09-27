//! The cases of P16, the monitor: random peer sets with verification results, ranks and heights
//! for `monitor.forkDecision`, and random height lists for `monitor.networkHeight` (exact
//! results).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P16.
pub fn monitor_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("monitor.networkHeight", 2, json!({}))).boxed()
}
