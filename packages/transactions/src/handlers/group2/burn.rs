//! 2/0 burn: an amount taken out of the supply.
//!
//! Checks: the amount is at least the milestone's `burn.txAmount`. Apply and revert: the base
//! ones, with the burned amount as the transaction's amount.

use heartwood_crypto::Transaction;
use heartwood_state::ApplyContext;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;

use crate::errors::TransactionError;
use crate::handlers::transaction::{self, Sender};

/// Run the type's checks, then the base checks, without changing `wallets`.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
) -> Result<(), TransactionError> {
    let minimum = ctx.params.burn_tx_amount();
    let amount = transaction.amount();
    if amount < minimum {
        return Err(TransactionError::InsufficientBurnAmount { amount, minimum });
    }
    let sender = Sender::of(ctx, wallets, transaction);
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction)?;
    transaction::apply_to_sender(ctx, wallets, transaction)?;
    Ok(TxUndo::None)
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    _undo: &TxUndo,
) -> Result<(), TransactionError> {
    transaction::revert_for_sender(ctx, wallets, transaction)?;
    Ok(())
}

/// The genesis path: bind the sender's key. Returns 0: the sender owes nothing beyond the amount
/// and the fee, which the caller takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
) -> Result<u64, TransactionError> {
    transaction::bind_sender_key(ctx, wallets, transaction);
    Ok(0)
}
