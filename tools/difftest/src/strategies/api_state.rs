//! P21, the listings over the state: random wallet sets (attributes, balances with ties, resigned
//! delegates, usernames with digits and punctuation) in random index orders, random criteria,
//! `orderBy` chains and pages, for `api.delegatesPage` and `api.walletsPage`.

use std::sync::Arc;

use proptest::strategy::BoxedStrategy;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P21 over the delegate and wallet listings.
pub fn listing_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the listings' cases")
}
