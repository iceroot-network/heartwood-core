//! P23, the query grammar: random raw queries over every route's schema (nested keys to the
//! grammar's 128 levels, comma lists, holes, casts, `%` patterns, percent-encoding and bytes beyond
//! ASCII, repeated keys, the paging keys), for `api.parseQuery` with the route's validation.

use std::sync::Arc;

use proptest::strategy::BoxedStrategy;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P23.
pub fn query_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the query grammar's cases")
}
