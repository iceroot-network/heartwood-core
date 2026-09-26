//! Vote balances.
//!
//! A voter's balance splits over the delegates it votes for: delegate `i` gets
//! `floor(balance × bps_i / 10,000)` (computed without overflow), and the units left over go one
//! each to the first delegates in the order the votes are kept. The wallet keeps its shares as it
//! last added them to its delegates' vote balances, so they can be taken back exactly.

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Votes;
use indexmap::IndexMap;

use crate::errors::WalletError;
use crate::wallets::wallet_repository::WalletRepository;

/// Each delegate's share of `balance` under `votes` (whose basis points add up to 10,000), in
/// the order of `votes`.
pub fn calculate_vote_amounts(balance: u64, votes: &Votes) -> IndexMap<String, u64> {
    let _ = (balance, votes);
    todo!()
}

/// Add the shares of the wallet at `address` to its delegates' vote balances, and one voter to
/// each when `update_voters` is set.
pub fn increase_vote_balances(
    wallets: &mut WalletRepository,
    address: &Address,
    update_voters: bool,
) -> Result<(), WalletError> {
    let _ = (wallets, address, update_voters);
    todo!()
}

/// Take the shares of the wallet at `address` from its delegates' vote balances, and one voter
/// from each when `update_voters` is set.
pub fn decrease_vote_balances(
    wallets: &mut WalletRepository,
    address: &Address,
    update_voters: bool,
) -> Result<(), WalletError> {
    let _ = (wallets, address, update_voters);
    todo!()
}

/// Recompute the shares of the wallet at `address` from its current balance and votes, without
/// touching the delegates (the reference's `Wallet.updateVoteBalances`).
pub fn update_vote_shares(
    wallets: &mut WalletRepository,
    address: &Address,
) -> Result<(), WalletError> {
    let _ = (wallets, address);
    todo!()
}

/// Bring the delegates of the wallet at `address` up to date with its balance: take its old
/// shares, recompute them, add the new ones (the reference's `BlockState.updateWalletVoteBalance`).
pub fn update_wallet_vote_balance(
    wallets: &mut WalletRepository,
    address: &Address,
) -> Result<(), WalletError> {
    let _ = (wallets, address);
    todo!()
}
