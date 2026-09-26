//! Delegated proof of stake: the delegate ranking and the delegates of a round.

#[expect(
    clippy::module_inception,
    reason = "the module keeps the reference's file name, dpos/dpos"
)]
pub mod dpos;

pub use self::dpos::{build_delegate_ranking, build_vote_balances, set_delegates_round};
