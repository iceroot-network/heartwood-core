//! 1/6 transfer: one or more items to recipients.
//!
//! Checks: the balance must cover the items and the fee. Apply: take the items' sum from the
//! sender and credit each item to its recipient, a repeated recipient once per item. Revert: the
//! inverse. At genesis the items are credited, and the sender owes their sum unless the transfer
//! is an issuance record.

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

/// The genesis path: bind the sender's key and credit the items. Returns what the sender owes
/// for the items: their sum, or 0 when the transfer is an issuance record.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    issuance: bool,
) -> Result<u64, TransactionError> {
    let _ = (ctx, wallets, transaction, issuance);
    todo!()
}
