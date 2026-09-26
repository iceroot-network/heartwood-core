//! 2/2 vote: delegate names with basis points that add up to 10,000; empty to remove every
//! vote.
//!
//! Checks, in the reference's order: at most `activeDelegates` entries; votes equal to the
//! wallet's current votes are refused (as having no vote when both are empty); every name is a
//! registered delegate (resigned delegates included). Apply: take the sender's shares from its
//! delegates, keep the votes in the canonical order, recompute the shares from the balance after
//! the fee and add them. Revert: the same with the previous votes of the undo record. Pool entry:
//! one vote per sender in the pool, and no vote for a delegate that neither resigned nor operates
//! a node.

use heartwood_crypto::Transaction;
use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Votes;
use heartwood_state::ApplyContext;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::vote_balances::{
    decrease_vote_balances, increase_vote_balances, update_vote_shares,
};
use heartwood_state::wallets::{Wallet, WalletRepository};

use crate::errors::{PoolError, TransactionError};
use crate::handlers::transaction::{self, Sender, Snapshot};
use crate::pool_query::{OnlineDelegates, PoolQuery};

/// Whether `a` and `b` hold the same names with the same basis points, in any order.
fn same_votes(a: &Votes, b: &Votes) -> bool {
    a.len() == b.len() && a.iter().all(|(name, bps)| b.get(name) == Some(bps))
}

/// Run the type's checks, then the base checks, without changing `wallets`. `votes` is the
/// vote asset.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    votes: &Votes,
) -> Result<(), TransactionError> {
    let maximum = ctx.params.active_delegates();
    if u64::try_from(votes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(TransactionError::VotedForTooManyDelegates { maximum });
    }
    let sender = Sender::of(ctx, wallets, transaction);
    if same_votes(votes, sender.wallet.votes()) {
        return Err(if votes.is_empty() {
            TransactionError::NoVote
        } else {
            TransactionError::AlreadyVotedForSameDelegates
        });
    }
    if let Some(name) = votes.keys().find(|name| !wallets.has_by_username(name)) {
        return Err(TransactionError::VotedForNonDelegate { vote: name.clone() });
    }
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// Replace the votes of the wallet at `address` with `votes`: take its shares from its current
/// delegates (one voter less each), set the votes in the canonical order, recompute the shares
/// from the balance and add them to the new delegates (one voter more each).
fn replace_votes(
    wallets: &mut WalletRepository,
    address: &Address,
    votes: &Votes,
) -> Result<(), TransactionError> {
    decrease_vote_balances(wallets, address, true)?;
    wallets.get_or_create(address).set_votes(votes);
    update_vote_shares(wallets, address)?;
    increase_vote_balances(wallets, address, true)?;
    Ok(())
}

/// The sender and the delegates of its current votes and of `votes`: the wallets a vote or its
/// revert changes.
fn touched(wallets: &WalletRepository, sender: Address, votes: &Votes) -> Vec<Address> {
    let current = wallets
        .find_by_address(&sender)
        .map(|wallet| wallet.votes().keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let delegates = current
        .iter()
        .chain(votes.keys())
        .filter_map(|name| wallets.find_by_username(name))
        .map(|wallet| *wallet.address());
    std::iter::once(sender).chain(delegates).collect()
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    votes: &Votes,
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction, votes)?;
    let sender = transaction.data().sender_address(ctx.params);
    let previous = wallets
        .find_by_address(&sender)
        .map(|wallet| wallet.votes().clone())
        .unwrap_or_default();
    Snapshot::take(wallets, &touched(wallets, sender, votes)).guard(wallets, |wallets| {
        let address = transaction::apply_to_sender(ctx, wallets, transaction)?;
        replace_votes(wallets, &address, votes)?;
        Ok(TxUndo::Vote { previous })
    })
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
) -> Result<(), TransactionError> {
    let TxUndo::Vote { previous } = undo else {
        return Err(TransactionError::UndoMismatch);
    };
    let sender = transaction.data().sender_address(ctx.params);
    Snapshot::take(wallets, &touched(wallets, sender, previous)).guard(wallets, |wallets| {
        let address = transaction::revert_for_sender(ctx, wallets, transaction)?;
        replace_votes(wallets, &address, previous)
    })
}

/// The genesis path: bind the sender's key and set its votes in the canonical order; the vote
/// balances are built once every genesis transaction is in. Returns 0: the sender owes nothing
/// beyond the amount and the fee, which the caller takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    votes: &Votes,
) -> Result<u64, TransactionError> {
    let address = transaction::bind_sender_key(ctx, wallets, transaction);
    wallets.get_or_create(&address).set_votes(votes);
    Ok(0)
}

/// Refuse `transaction` at pool entry when the sender already has a vote in the pool
/// (`ERR_PENDING`), or it votes for a delegate that has not resigned and whose node is offline
/// (`ERR_OFFLINE`), the first such name in the order of the vote.
pub fn throw_if_cannot_enter_pool<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    votes: &Votes,
    pool: &P,
    online: &O,
) -> Result<(), PoolError> {
    let sender = transaction.data().sender_address(ctx.params);
    if pool.has_sender_kind(&sender, transaction.kind()) {
        return Err(PoolError::SenderPending {
            sender,
            kind: transaction.kind(),
        });
    }
    let offline = votes.keys().find(|name| {
        wallets
            .find_by_username(name)
            .and_then(Wallet::delegate)
            .is_some_and(|delegate| delegate.resigned.is_none() && !online.is_online(name))
    });
    match offline {
        Some(username) => Err(PoolError::Offline {
            username: username.clone(),
        }),
        None => Ok(()),
    }
}
