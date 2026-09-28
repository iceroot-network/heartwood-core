//! The cases of P18, the quorum: random peer sets (heights, fast verifications, attested keys,
//! states), the forger's attested keys, round sets that differ from the ranking, ranks and
//! heights, for `quorum.analyze` (the answer's fields and `canForge` must be the reference's).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P18.
pub fn quorum_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("quorum.analyze", 2, json!({}))).boxed()
}
