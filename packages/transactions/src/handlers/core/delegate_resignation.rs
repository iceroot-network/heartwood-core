//! 1/7 delegate resignation: temporary (type 0, also when the byte is absent), permanent (1), or
//! the revoke of a temporary one (2).
//!
//! Checks, in the reference's order: the sender is a delegate; a permanently resigned delegate
//! may not resign again or revoke; a temporarily resigned one may not resign temporarily again,
//! and may revoke only once `blocksToRevokeDelegateResignation` blocks have passed since its
//! resignation; a delegate that has not resigned may not revoke; a resignation may not leave fewer
//! unresigned delegates than a round needs. Apply: set or forget the resignation and record its
//! height. Revert: restore both from the undo record. Pool entry: one resignation per sender in
//! the pool, and a revoke only by a delegate that operates a node.

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::types::ResignationType;
use heartwood_state::ApplyContext;
use heartwood_state::errors::WalletError;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::{Delegate, Resignation, Wallet, WalletRepository};

use crate::errors::{PoolError, TransactionError};
use crate::handlers::transaction::{self, Sender};
use crate::pool_query::{OnlineDelegates, PoolQuery};

/// What a resignation of `resignation_type` asks for: `Some` resignation, or `None` for a revoke.
/// No resignation byte means a temporary resignation.
fn requested(resignation_type: Option<ResignationType>) -> Option<Resignation> {
    match resignation_type {
        None | Some(ResignationType::TemporaryResign) => Some(Resignation::Temporary),
        Some(ResignationType::PermanentResign) => Some(Resignation::Permanent),
        Some(ResignationType::NotResigned) => None,
    }
}

/// Run the type's checks, then the base checks, without changing `wallets`.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    resignation_type: Option<ResignationType>,
) -> Result<(), TransactionError> {
    let sender = Sender::of(ctx, wallets, transaction);
    let Some(delegate) = sender.wallet.delegate() else {
        return Err(TransactionError::WalletNotADelegate);
    };
    let requested = requested(resignation_type);
    match (delegate.resigned, requested) {
        (Some(Resignation::Permanent), Some(Resignation::Permanent)) => {
            return Err(TransactionError::WalletAlreadyPermanentlyResigned);
        }
        (Some(Resignation::Permanent), _) => {
            return Err(TransactionError::IrrevocableResignation);
        }
        (Some(Resignation::Temporary), Some(Resignation::Temporary)) => {
            return Err(TransactionError::WalletAlreadyTemporarilyResigned);
        }
        (Some(Resignation::Temporary), None) => check_revoke_delay(ctx, delegate)?,
        (None, None) => return Err(TransactionError::WalletNotResigned),
        (Some(Resignation::Temporary), Some(Resignation::Permanent)) | (None, Some(_)) => {}
    }
    if delegate.resigned.is_none() {
        let unresigned = wallets
            .delegates()
            .filter_map(Wallet::delegate)
            .filter(|delegate| delegate.resigned.is_none())
            .count();
        let remaining = u64::try_from(unresigned).unwrap_or(u64::MAX).checked_sub(1);
        if remaining.is_none_or(|remaining| remaining < ctx.params.active_delegates()) {
            return Err(TransactionError::NotEnoughDelegates);
        }
    }
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// A revoke waits `blocksToRevokeDelegateResignation` blocks after the delegate's latest
/// resignation transaction up to the last block. The reference looks that transaction up first
/// and fails when it finds none; with no delay configured it compares with `undefined`, which
/// never holds a revoke back.
fn check_revoke_delay(ctx: &ApplyContext<'_>, delegate: &Delegate) -> Result<(), TransactionError> {
    let last = ctx.last_block_height;
    let resigned_at = delegate
        .resigned_at
        .filter(|height| *height <= last)
        .ok_or(TransactionError::ResignationNotFound)?;
    let since = u64::from(last.saturating_sub(resigned_at));
    if let Some(delay) = ctx.params.blocks_to_revoke_delegate_resignation()
        && since < delay
    {
        return Err(TransactionError::NotEnoughTimeSinceResignation {
            blocks: delay.saturating_sub(since),
        });
    }
    Ok(())
}

/// Set `resigned` and its height on the delegate of the wallet at `address`, returning the values
/// they had.
fn set_resignation(
    wallets: &mut WalletRepository,
    address: &heartwood_crypto::identities::Address,
    resigned: Option<Resignation>,
    resigned_at: Option<u32>,
) -> Result<(Option<Resignation>, Option<u32>), TransactionError> {
    let delegate = wallets
        .wallet_mut(address)
        .and_then(Wallet::delegate_mut)
        .ok_or(WalletError::NotADelegate { address: *address })?;
    let previous = (delegate.resigned, delegate.resigned_at);
    delegate.resigned = resigned;
    delegate.resigned_at = resigned_at;
    Ok(previous)
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    resignation_type: Option<ResignationType>,
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction, resignation_type)?;
    let address = transaction::apply_to_sender(ctx, wallets, transaction)?;
    let (previous, previous_height) = set_resignation(
        wallets,
        &address,
        requested(resignation_type),
        Some(ctx.height),
    )?;
    Ok(TxUndo::Resignation {
        previous,
        previous_height,
    })
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
) -> Result<(), TransactionError> {
    let TxUndo::Resignation {
        previous,
        previous_height,
    } = undo
    else {
        return Err(TransactionError::UndoMismatch);
    };
    let address = transaction.data().sender_address(ctx.params);
    if wallets
        .find_by_address(&address)
        .and_then(Wallet::delegate)
        .is_none()
    {
        return Err(WalletError::NotADelegate { address }.into());
    }
    transaction::revert_for_sender(ctx, wallets, transaction)?;
    set_resignation(wallets, &address, *previous, *previous_height)?;
    Ok(())
}

/// The genesis path: bind the sender's key and set or forget the resignation, recording the
/// height. Returns 0: the sender owes nothing beyond the amount and the fee, which the caller
/// takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    resignation_type: Option<ResignationType>,
) -> Result<u64, TransactionError> {
    let address = transaction.data().sender_address(ctx.params);
    if wallets
        .find_by_address(&address)
        .and_then(Wallet::delegate)
        .is_none()
    {
        return Err(WalletError::NotADelegate { address }.into());
    }
    transaction::bind_sender_key(ctx, wallets, transaction);
    set_resignation(
        wallets,
        &address,
        requested(resignation_type),
        Some(ctx.height),
    )?;
    Ok(0)
}

/// Refuse `transaction` at pool entry when the sender already has a resignation in the pool
/// (`ERR_PENDING`), or it revokes while its node is offline (`ERR_OFFLINE`).
#[doc(alias = "throwIfCannotEnterPool")]
pub fn check_pool_entry<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    resignation_type: Option<ResignationType>,
    pool: &P,
    online: &O,
) -> Result<(), PoolError> {
    let sender = transaction.data().sender_address(ctx.params);
    let delegate = wallets.find_by_address(&sender).and_then(Wallet::delegate);
    if pool.has_sender_kind(&sender, transaction.kind()) {
        // A resignation enters the pool only from a delegate, so a pending one has a name.
        return Err(PoolError::ResignationPending {
            username: delegate.map_or_else(String::new, |d| d.username().to_owned()),
        });
    }
    if resignation_type == Some(ResignationType::NotResigned)
        && let Some(delegate) = delegate
        && !online.is_online(delegate.username())
    {
        return Err(PoolError::Offline {
            username: delegate.username().to_owned(),
        });
    }
    Ok(())
}
