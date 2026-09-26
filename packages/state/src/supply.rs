//! The supply check: the balances of all wallets add up to what the genesis block issued plus
//! every reward since, less every burned fee and every burned amount.
//!
//! The reference has no such check at run time; the replays run it after every step, so a rule
//! that creates or destroys value shows at the height where it does.

use crate::errors::SupplyError;
use crate::machine::State;

/// Check the supply of `state`.
pub fn check_supply(state: &State) -> Result<(), SupplyError> {
    let _ = state;
    todo!()
}
