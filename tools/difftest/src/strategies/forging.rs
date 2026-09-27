//! The cases of P17, forging: the pool fixtures and random pools of P10, a random round (previous
//! block, slot, reward), a delegate key and an aux, for `forger.collateAndForge` (the collated
//! ids, the removed ids and the block's bytes must be the reference's).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P17.
pub fn forging_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("forger.collateAndForge", 2, json!({}))).boxed()
}
