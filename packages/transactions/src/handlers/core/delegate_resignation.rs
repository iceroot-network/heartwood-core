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

/// Refuse `transaction` at pool entry when the sender already has a resignation in the pool
/// (`ERR_PENDING`), or it revokes while its node is offline (`ERR_OFFLINE`).
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
