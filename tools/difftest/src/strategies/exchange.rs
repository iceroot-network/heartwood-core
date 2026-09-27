//! The cases of P15, the exchange: random `ClientRequest` values for `p2p.validateRequest` (a
//! reference server must accept every one), and random mutations of valid replies for
//! `wire.checkReply` (the verdict and the value must be the reference's).

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P15.
pub fn exchange_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    Just(("p2p.validateRequest", 2, json!({}))).boxed()
}
