//! Vote balances.
//!
//! A voter's balance splits over the delegates it votes for: delegate `i` gets
//! `floor(balance × bps_i / 10,000)` (computed without overflow), and the units left over go one
//! each to the first delegates in the order the votes are kept. The wallet keeps its shares as it
//! last added them to its delegates' vote balances, so they can be taken back exactly.
//!
//! Every function here changes the delegates it touches all at once: it computes every new vote
//! balance and voter count first, and writes nothing when one of them is out of range or names a
//! delegate that does not exist.

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Votes;
use indexmap::IndexMap;

use crate::errors::WalletError;
use crate::wallets::wallet::Wallet;
use crate::wallets::wallet_repository::WalletRepository;

/// The basis points of a whole balance.
const WHOLE: u128 = 10_000;

/// Each delegate's share of `balance` under `votes`, in the order of `votes`.
///
/// Delegate `i` gets `floor(balance × bps_i / 10,000)`; what is left of `balance` goes one unit
/// each to the first delegates. The votes a wallet can hold add up to exactly 10,000 basis points
/// (or are empty), so fewer units are left than there are delegates. Votes that add up to less
/// hand out at most one unit per delegate; votes that add up to more hand out none.
pub fn calculate_vote_amounts(balance: u64, votes: &Votes) -> IndexMap<String, u64> {
    let mut amounts: IndexMap<String, u64> = votes
        .iter()
        .map(|(name, &basis_points)| (name.clone(), share(balance, basis_points)))
        .collect();
    let given = amounts.values().fold(0_u128, |sum, &amount| {
        sum.saturating_add(u128::from(amount))
    });
    let left = u128::from(balance).saturating_sub(given);
    let left = usize::try_from(left).unwrap_or(usize::MAX);
    for amount in amounts.values_mut().take(left) {
        *amount = amount.saturating_add(1);
    }
    amounts
}

/// `floor(balance × basis_points / 10,000)`, exact: the product of a `u64` and a `u16` fits
/// `u128`, and with at most 10,000 basis points the quotient fits `u64`.
fn share(balance: u64, basis_points: u16) -> u64 {
    let product = u128::from(balance).saturating_mul(u128::from(basis_points));
    let quotient = product.checked_div(WHOLE).unwrap_or_default();
    u64::try_from(quotient).unwrap_or(u64::MAX)
}

/// Add the shares of the wallet at `address` to its delegates' vote balances, and one voter to
/// each when `update_voters` is set. Nothing happens for a wallet that does not exist.
pub fn increase_vote_balances(
    wallets: &mut WalletRepository,
    address: &Address,
    update_voters: bool,
) -> Result<(), WalletError> {
    let mut staged = Staged::default();
    if let Some(wallet) = wallets.find_by_address(address) {
        for (username, &amount) in wallet.vote_shares() {
            staged.change(wallets, username, amount, Direction::Add, update_voters)?;
        }
    }
    staged.commit(wallets);
    Ok(())
}

/// Take the shares of the wallet at `address` from its delegates' vote balances, and one voter
/// from each when `update_voters` is set. Nothing happens for a wallet that does not exist.
pub fn decrease_vote_balances(
    wallets: &mut WalletRepository,
    address: &Address,
    update_voters: bool,
) -> Result<(), WalletError> {
    let mut staged = Staged::default();
    if let Some(wallet) = wallets.find_by_address(address) {
        for (username, &amount) in wallet.vote_shares() {
            staged.change(wallets, username, amount, Direction::Take, update_voters)?;
        }
    }
    staged.commit(wallets);
    Ok(())
}

/// Recompute the shares of the wallet at `address` from its current balance and votes, without
/// touching the delegates (the reference's `Wallet.updateVoteBalances`). Nothing happens for a
/// wallet that does not exist.
pub fn update_vote_shares(
    wallets: &mut WalletRepository,
    address: &Address,
) -> Result<(), WalletError> {
    let Some(wallet) = wallets.find_by_address(address) else {
        return Ok(());
    };
    let shares = calculate_vote_amounts(wallet.balance(), wallet.votes());
    if wallet.vote_shares() != &shares {
        set_shares(wallets, address, shares);
    }
    Ok(())
}

/// Bring the delegates of the wallet at `address` up to date with its balance: take its old
/// shares, recompute them, add the new ones (the reference's `BlockState.updateWalletVoteBalance`).
/// Nothing happens for a wallet that does not exist.
pub fn update_wallet_vote_balance(
    wallets: &mut WalletRepository,
    address: &Address,
) -> Result<(), WalletError> {
    let Some(wallet) = wallets.find_by_address(address) else {
        return Ok(());
    };
    let shares = calculate_vote_amounts(wallet.balance(), wallet.votes());
    let unchanged = wallet.vote_shares() == &shares;
    // Every delegate is looked up even when the shares stay the same, as the reference takes and
    // adds them in any case; the values it writes back are then the same.
    let mut staged = Staged::default();
    for (username, &amount) in wallet.vote_shares() {
        staged.change(wallets, username, amount, Direction::Take, false)?;
    }
    for (username, &amount) in &shares {
        staged.change(wallets, username, amount, Direction::Add, false)?;
    }
    staged.commit(wallets);
    if !unchanged {
        set_shares(wallets, address, shares);
    }
    Ok(())
}

fn set_shares(wallets: &mut WalletRepository, address: &Address, shares: IndexMap<String, u64>) {
    if let Some(wallet) = wallets.wallet_mut(address) {
        wallet.set_vote_shares(shares);
    }
}

/// Whether a share is added to a delegate's vote balance or taken from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Add,
    Take,
}

/// New vote balances and voter counts by delegate address, computed before any is written.
#[derive(Debug, Default)]
struct Staged {
    values: IndexMap<Address, (u64, u64)>,
}

impl Staged {
    /// Stage `amount` added to or taken from the vote balance of the delegate `username`, and one
    /// voter with it when `count_voter` is set.
    fn change(
        &mut self,
        wallets: &WalletRepository,
        username: &str,
        amount: u64,
        direction: Direction,
        count_voter: bool,
    ) -> Result<(), WalletError> {
        let address = *wallets.indexes().by_username(username).ok_or_else(|| {
            WalletError::UnknownDelegate {
                username: username.to_owned(),
            }
        })?;
        let (vote_balance, voters) = match self.values.get(&address) {
            Some(&values) => values,
            None => {
                let delegate = wallets
                    .find_by_address(&address)
                    .and_then(Wallet::delegate)
                    .ok_or(WalletError::NotADelegate { address })?;
                (delegate.vote_balance, delegate.voters)
            }
        };
        let voter = u64::from(count_voter);
        let changed = match direction {
            Direction::Add => vote_balance
                .checked_add(amount)
                .zip(voters.checked_add(voter)),
            Direction::Take => vote_balance
                .checked_sub(amount)
                .zip(voters.checked_sub(voter)),
        };
        let changed = changed.ok_or_else(|| WalletError::DelegateRange {
            username: username.to_owned(),
        })?;
        self.values.insert(address, changed);
        Ok(())
    }

    /// Write the staged values; a delegate whose values did not change is left alone, so a wallet
    /// shared with another clone of the repository is not copied for nothing.
    fn commit(self, wallets: &mut WalletRepository) {
        for (address, (vote_balance, voters)) in self.values {
            let unchanged = wallets
                .find_by_address(&address)
                .and_then(Wallet::delegate)
                .is_some_and(|d| d.vote_balance == vote_balance && d.voters == voters);
            if unchanged {
                continue;
            }
            if let Some(delegate) = wallets.wallet_mut(&address).and_then(Wallet::delegate_mut) {
                delegate.vote_balance = vote_balance;
                delegate.voters = voters;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn votes(entries: &[(&str, u16)]) -> Votes {
        entries
            .iter()
            .map(|&(name, basis_points)| (name.to_owned(), basis_points))
            .collect()
    }

    fn amounts(balance: u64, entries: &[(&str, u16)]) -> Vec<(String, u64)> {
        calculate_vote_amounts(balance, &votes(entries))
            .into_iter()
            .collect()
    }

    #[test]
    fn the_remainder_goes_to_the_first_entries_in_map_order() {
        assert_eq!(
            amounts(101, &[("a10", 5_000), ("a9", 5_000)]),
            [("a10".to_owned(), 51), ("a9".to_owned(), 50)]
        );
        assert_eq!(
            amounts(101, &[("a9", 5_000), ("a10", 5_000)]),
            [("a9".to_owned(), 51), ("a10".to_owned(), 50)]
        );
        assert_eq!(
            amounts(2, &[("a", 3_334), ("b", 3_333), ("c", 3_333)]),
            [
                ("a".to_owned(), 1),
                ("b".to_owned(), 1),
                ("c".to_owned(), 0)
            ]
        );
    }

    #[test]
    fn edges_are_exact() {
        assert!(calculate_vote_amounts(5, &Votes::new()).is_empty());
        assert_eq!(amounts(0, &[("a", 10_000)]), [("a".to_owned(), 0)]);
        assert_eq!(
            amounts(u64::MAX, &[("a", 10_000)]),
            [("a".to_owned(), u64::MAX)]
        );
        let split = amounts(u64::MAX, &[("a", 1), ("b", 9_999)]);
        assert_eq!(split[0].1, u64::MAX / 10_000 + 1);
        assert_eq!(split[1].1, u64::MAX - split[0].1);
    }

    #[test]
    fn votes_off_the_domain_hand_out_at_most_one_unit_each() {
        assert_eq!(
            amounts(100, &[("a", 1_000), ("b", 1_000)]),
            [("a".to_owned(), 11), ("b".to_owned(), 11)]
        );
        assert_eq!(
            amounts(100, &[("a", 9_000), ("b", 9_000)]),
            [("a".to_owned(), 90), ("b".to_owned(), 90)]
        );
        assert_eq!(
            amounts(u64::MAX, &[("a", u16::MAX)]),
            [("a".to_owned(), u64::MAX)]
        );
    }
}
