//! The delegate ranking, the delegates of a round, and the vote balances built at start.

use heartwood_crypto::identities::{Address, PublicKey};
use heartwood_kernel::RoundInfo;

use crate::errors::{RoundError, WalletError};
use crate::round_state::RoundDelegate;
use crate::wallets::vote_balances::{increase_vote_balances, update_vote_shares};
use crate::wallets::{Wallet, WalletRepository};

/// A delegate that takes part in the ranking.
struct Candidate<'a> {
    address: Address,
    username: &'a str,
    vote_balance: u64,
    public_key: Option<&'a [u8]>,
}

/// Rank every registered delegate, as the reference's `buildDelegateRanking` does: resigned
/// delegates lose their rank; the others are ranked from 1 by vote balance, highest first, then by
/// public key bytes, lowest first. Returns the ranked delegates' addresses in rank order.
///
/// The public keys decide only between equal vote balances, and the reference compares their
/// lowercase hex with the `en` collation, which for hex is the order of the bytes. Two delegates
/// with the same vote balance and the same key, or one without a key among equal vote balances,
/// refuse the ranking, as the reference's comparison throws there. On an error no rank changes.
pub fn build_delegate_ranking(wallets: &mut WalletRepository) -> Result<Vec<Address>, RoundError> {
    let mut resigned = Vec::new();
    let mut candidates = Vec::new();
    for wallet in wallets.delegates() {
        let Some(delegate) = wallet.delegate() else {
            continue;
        };
        if delegate.resigned.is_some() {
            resigned.push(*wallet.address());
        } else {
            candidates.push(Candidate {
                address: *wallet.address(),
                username: delegate.username(),
                vote_balance: delegate.vote_balance,
                public_key: wallet.public_key().map(PublicKey::as_bytes),
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.vote_balance
            .cmp(&a.vote_balance)
            .then_with(|| a.public_key.cmp(&b.public_key))
    });
    for pair in candidates.windows(2) {
        if let [a, b] = pair
            && a.vote_balance == b.vote_balance
        {
            check_tie(a, b)?;
        }
    }
    if u32::try_from(candidates.len()).is_err() {
        let username = candidates.last().map(|c| c.username.to_owned());
        return Err(RoundError::Wallet(WalletError::DelegateRange {
            username: username.unwrap_or_default(),
        }));
    }
    let ranked: Vec<Address> = candidates.iter().map(|c| c.address).collect();

    for address in &resigned {
        set_rank(wallets, address, None);
    }
    for (rank, address) in (1..=u32::MAX).zip(&ranked) {
        set_rank(wallets, address, Some(rank));
    }
    Ok(ranked)
}

/// Two delegates with equal vote balances, next to each other in the ranking: both need a key,
/// and the keys must differ.
fn check_tie(a: &Candidate<'_>, b: &Candidate<'_>) -> Result<(), RoundError> {
    let no_key = |candidate: &Candidate<'_>| RoundError::NoPublicKey {
        username: candidate.username.to_owned(),
    };
    match (a.public_key, b.public_key) {
        (None, _) => Err(no_key(a)),
        (_, None) => Err(no_key(b)),
        (Some(x), Some(y)) if x == y => Err(RoundError::IdenticalPublicKeys {
            first: a.username.to_owned(),
            second: b.username.to_owned(),
        }),
        _ => Ok(()),
    }
}

/// Set the rank of the delegate at `address`, writing only when it changes, so a wallet shared
/// with another clone of the repository is not copied for nothing.
pub(crate) fn set_rank(wallets: &mut WalletRepository, address: &Address, rank: Option<u32>) {
    let current = wallets
        .find_by_address(address)
        .and_then(Wallet::delegate)
        .map(|delegate| delegate.rank);
    if current.is_some_and(|current| current != rank)
        && let Some(delegate) = wallets.wallet_mut(address).and_then(Wallet::delegate_mut)
    {
        delegate.rank = rank;
    }
}

/// The delegates of the round `round`: the first `round.max_delegates` of `ranked` (the ranking's
/// result), in rank order, with their vote balances. Fewer ranked delegates than the round needs
/// refuse the round, as the reference refuses to go on.
pub fn set_delegates_round(
    wallets: &WalletRepository,
    ranked: &[Address],
    round: &RoundInfo,
) -> Result<Vec<RoundDelegate>, RoundError> {
    let required = round.max_delegates;
    let not_enough = RoundError::NotEnoughDelegates {
        required,
        ranked: ranked.len(),
    };
    let count = usize::try_from(required).map_err(|_| not_enough.clone())?;
    let delegates = ranked.get(..count).ok_or(not_enough)?;
    delegates
        .iter()
        .map(|address| {
            let wallet = wallets
                .find_by_address(address)
                .ok_or(WalletError::UnknownWallet { address: *address })?;
            let delegate = wallet
                .delegate()
                .ok_or(WalletError::NotADelegate { address: *address })?;
            let public_key = wallet.public_key().ok_or_else(|| RoundError::NoPublicKey {
                username: delegate.username().to_owned(),
            })?;
            Ok(RoundDelegate {
                username: delegate.username().to_owned(),
                public_key: public_key.to_compressed(),
                vote_balance: delegate.vote_balance,
            })
        })
        .collect()
}

/// Build every delegate's vote balance and voter count from the voters, as the reference does at
/// start: every wallet with a public key and votes computes its shares and adds them, and one
/// voter, to each delegate it votes for.
pub fn build_vote_balances(wallets: &mut WalletRepository) -> Result<(), WalletError> {
    let voters: Vec<Address> = wallets
        .iter()
        .filter(|wallet| wallet.public_key().is_some() && wallet.has_voted())
        .map(|wallet| *wallet.address())
        .collect();
    for address in &voters {
        update_vote_shares(wallets, address)?;
        increase_vote_balances(wallets, address, true)?;
    }
    Ok(())
}
