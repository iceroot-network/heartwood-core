//! The `proptest` strategies of L1: the inputs of each property, in the domains the reference
//! and Heartwood both accept.
//!
//! Amounts stay inside the domain every implementation can represent: the balances of a fixture
//! add up to at most `2^63 − 1`, as a genesis block can issue at most that much.

use proptest::strategy::BoxedStrategy;

use crate::fixture::Fixture;

/// Heights from 1 to `2^32 − 1`, the edges of a round of `active_delegates` more often.
pub fn heights(active_delegates: u64) -> BoxedStrategy<u32> {
    let _ = active_delegates;
    todo!()
}

/// Wallet fixtures of 3 to 12 wallets: delegates, resigned delegates with their resignation
/// height, voters and second keys, with balances that add up to at most `2^63 − 1`.
pub fn fixtures() -> BoxedStrategy<Fixture> {
    todo!()
}
