//! The supply check: the balances of all wallets add up to what the genesis block issued plus
//! every reward since, less every burned fee and every burned amount.
//!
//! The reference has no such check at run time; the replays run it after every step, so a rule
//! that creates or destroys value shows at the height where it does.

use crate::errors::SupplyError;
use crate::machine::State;
use crate::wallets::WalletRepository;

/// Check the supply of `state`: the sum of every balance equals
/// [`crate::stores::Totals::expected_supply`]. A balance is a `u64`, so none can be negative.
pub fn check_supply(state: &State) -> Result<(), SupplyError> {
    let balances = total_balance(state.wallets());
    let expected = state.totals().expected_supply();
    if i128::try_from(balances).is_ok_and(|sum| sum == expected) {
        Ok(())
    } else {
        Err(SupplyError { balances, expected })
    }
}

/// The sum of every balance of `wallets`.
pub fn total_balance(wallets: &WalletRepository) -> u128 {
    // At most 2^64 wallets of at most 2^64 − 1 each: the sum fits a u128.
    wallets
        .iter()
        .map(|wallet| u128::from(wallet.balance()))
        .fold(0, u128::saturating_add)
}
