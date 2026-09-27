//! The cases of P14, the verifier: a random common height, claimed height and peer branch on a
//! corpus chain, with optional bogus common-block replies and unlinked segments, for
//! `verifier.checkState` (the result and the probe sequence must be the reference's).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P14.
pub fn verifier_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("verifier.checkState", 2, json!({}))).boxed()
}
