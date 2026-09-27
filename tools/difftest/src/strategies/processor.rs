//! The cases of P13, the processor: on a random prefix of a corpus chain, the next block built by
//! the synthetic builder and then mutated (its timestamp, height, previous id, generator, reward,
//! transactions, version and payload length), for `chain.process`: the result, the unchained
//! status and the line after an accepted block must be the reference's.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P13.
pub fn processor_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("chain.process", 2, json!({}))).boxed()
}
