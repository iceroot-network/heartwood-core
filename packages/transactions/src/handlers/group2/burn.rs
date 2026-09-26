//! 2/0 burn: an amount taken out of the supply.
//!
//! Checks: the amount is at least the milestone's `burn.txAmount`. Apply and revert: the base
//! ones, with the burned amount as the transaction's amount.

use heartwood_crypto::Transaction;
use heartwood_state::ApplyContext;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;

use crate::errors::TransactionError;

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
