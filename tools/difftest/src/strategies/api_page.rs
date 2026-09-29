//! P21's small-limit branch: limits 1 to 3 with random offsets over items of mixed JSON types and
//! plain objects at random positions, for `api.searchPage`.

use std::sync::Arc;

use proptest::strategy::BoxedStrategy;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P21's small-limit branch.
pub fn page_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the small-limit cases")
}
