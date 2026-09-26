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
use crate::pool_query::{OnlineDelegates, PoolQuery};

/// Run the type's checks, then the base checks, without changing `wallets`.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
) -> Result<(), TransactionError> {
    let _ = (ctx, wallets, transaction);
    todo!()
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<TxUndo, TransactionError> {
    let _ = (ctx, wallets, transaction);
    todo!()
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
) -> Result<(), TransactionError> {
    let _ = (ctx, wallets, transaction, undo);
    todo!()
}

/// The genesis path: bind the sender's key and set the type's effect. Returns 0: the sender owes
/// nothing beyond the amount and the fee, which the caller takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<u64, TransactionError> {
    let _ = (ctx, wallets, transaction);
    todo!()
}

/// Refuse `transaction` at pool entry when the sender, or the name, already has a delegate
/// registration in the pool: `ERR_PENDING`.
pub fn throw_if_cannot_enter_pool<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    pool: &P,
    online: &O,
) -> Result<(), PoolError> {
    let _ = (ctx, wallets, transaction, pool, online);
    todo!()
}
