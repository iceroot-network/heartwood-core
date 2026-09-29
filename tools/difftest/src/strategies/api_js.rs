//! P22, the wallet search: random wallet sets in random address-index orders and random inputs
//! (short usernames, long address and key prefixes, mixed case), for `api.walletsLike`.

use std::sync::Arc;

use proptest::strategy::BoxedStrategy;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The cases of P22.
pub fn wallet_search_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the wallet search's cases")
}
