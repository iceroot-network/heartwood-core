//! The delegate ranking, the delegates of a round, and the vote balances built at start.

use heartwood_crypto::identities::Address;
use heartwood_kernel::RoundInfo;

use crate::errors::{RoundError, WalletError};
use crate::round_state::RoundDelegate;
use crate::wallets::WalletRepository;

/// Rank every registered delegate, as the reference's `buildDelegateRanking` does: resigned
/// delegates lose their rank; the others are ranked from 1 by vote balance, highest first, then by
/// public key bytes, lowest first. Returns the ranked delegates' addresses in rank order.
pub fn build_delegate_ranking(wallets: &mut WalletRepository) -> Result<Vec<Address>, RoundError> {
    let _ = wallets;
    todo!()
}

/// The delegates of the round `round`: the first `round.max_delegates` of `ranked` (the ranking's
/// result), in rank order, with their vote balances.
pub fn set_delegates_round(
    wallets: &WalletRepository,
    ranked: &[Address],
    round: &RoundInfo,
) -> Result<Vec<RoundDelegate>, RoundError> {
    let _ = (wallets, ranked, round);
    todo!()
}

/// Build every delegate's vote balance and voter count from the voters, as the reference does at
/// start: every wallet with a public key and votes computes its shares and adds them, and one
/// voter, to each delegate it votes for.
pub fn build_vote_balances(wallets: &mut WalletRepository) -> Result<(), WalletError> {
    let _ = wallets;
    todo!()
}
