//! 1/2 delegate registration.
//!
//! Checks: the sender is not a delegate yet; no delegate has the name. Apply: make the sender a
//! delegate with the name (every counter 0, no rank) and index the name. Revert: forget the
//! delegate and the name. Pool entry: one registration per sender, and per name, in the pool.

use heartwood_crypto::Transaction;
use heartwood_state::ApplyContext;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;

use crate::errors::{PoolError, TransactionError};
use crate::handlers::transaction::{self, Sender};
use crate::pool_query::{OnlineDelegates, PoolQuery};

/// Run the type's checks, then the base checks, without changing `wallets`. `username` is the
/// name to register.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    username: &str,
) -> Result<(), TransactionError> {
    let sender = Sender::of(ctx, wallets, transaction);
    if sender.wallet.is_delegate() {
        return Err(TransactionError::WalletIsAlreadyDelegate);
    }
    if wallets.has_by_username(username) {
        return Err(TransactionError::WalletUsernameAlreadyRegistered {
            username: username.to_owned(),
        });
    }
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    username: &str,
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction, username)?;
    let address = transaction::apply_to_sender(ctx, wallets, transaction)?;
    // The check found the name free, so the registration cannot be refused.
    wallets.register_delegate(&address, username.to_owned())?;
    Ok(TxUndo::None)
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    _undo: &TxUndo,
) -> Result<(), TransactionError> {
    let address = transaction::revert_for_sender(ctx, wallets, transaction)?;
    wallets.forget_delegate(&address);
    Ok(())
}

/// The genesis path: bind the sender's key and make the sender a delegate with the name.
/// Returns 0: the sender owes nothing beyond the amount and the fee, which the caller takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    username: &str,
) -> Result<u64, TransactionError> {
    let address = transaction.data().sender_address(ctx.params);
    // Refuse before the key is bound, so that a refusal changes nothing.
    if wallets
        .find_by_username(username)
        .is_some_and(|holder| holder.address() != &address)
    {
        return Err(TransactionError::WalletUsernameAlreadyRegistered {
            username: username.to_owned(),
        });
    }
    transaction::bind_sender_key(ctx, wallets, transaction);
    wallets.register_delegate(&address, username.to_owned())?;
    Ok(0)
}

/// Refuse `transaction` at pool entry when the sender, or the name, already has a delegate
/// registration in the pool: `ERR_PENDING`.
#[doc(alias = "throwIfCannotEnterPool")]
pub fn check_pool_entry<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    _wallets: &WalletRepository,
    transaction: &Transaction,
    username: &str,
    pool: &P,
    _online: &O,
) -> Result<(), PoolError> {
    let sender = transaction.data().sender_address(ctx.params);
    if pool.has_sender_kind(&sender, transaction.kind()) {
        return Err(PoolError::SenderPending {
            sender,
            kind: transaction.kind(),
        });
    }
    if pool.has_pending_username(username) {
        return Err(PoolError::UsernamePending {
            username: username.to_owned(),
        });
    }
    Ok(())
}
