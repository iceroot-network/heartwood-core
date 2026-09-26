//! A block's transactions and its forger's payout, applied and reverted, as the reference's
//! `BlockState` does.
//!
//! Applying a transaction runs its handler, then brings the vote balances up to date for every
//! wallet whose balance it changed: each transfer recipient in item order, then the sender.
//! Applying a block applies its transactions in order with the rules of the block's height; if
//! one is refused, the ones before it are reverted in reverse order and the error is returned,
//! so a refused block leaves the wallets as they were. Then the forger is paid: the reward less
//! the donations, plus the fees less their burned part; the donations go to their recipients.

use heartwood_crypto::{Block, Milestones, Transaction};

use crate::errors::{ApplyError, RevertError};
use crate::handlers::{ApplyContext, TransactionHandlers, TransactionRejected};
use crate::stores::state::Totals;
use crate::undo::{BlockUndo, TxUndo};
use crate::wallets::WalletRepository;

/// Apply `transaction` with `handlers`, then update the vote balances of the wallets it changed.
pub fn apply_transaction<H: TransactionHandlers>(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    handlers: &H,
) -> Result<TxUndo, TransactionRejected> {
    let _ = (ctx, wallets, transaction, handlers);
    todo!()
}

/// Revert `transaction` with `handlers` and its undo record, then update the vote balances of
/// the wallets it changed.
pub fn revert_transaction<H: TransactionHandlers>(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
    handlers: &H,
) -> Result<(), TransactionRejected> {
    let _ = (ctx, wallets, transaction, undo, handlers);
    todo!()
}

/// Apply the transactions of `block` and pay its forger, adding the reward and the burned fees
/// and amounts to `totals`. Returns the block's undo record without a round part, which the
/// round state adds. On an error the wallets and `totals` are unchanged.
pub fn apply_block<H: TransactionHandlers>(
    wallets: &mut WalletRepository,
    totals: &mut Totals,
    block: &Block,
    milestones: &Milestones,
    handlers: &H,
) -> Result<BlockUndo, ApplyError> {
    let _ = (wallets, totals, block, milestones, handlers);
    todo!()
}

/// Revert the forger's payout of the block of `undo`, then its transactions in reverse order,
/// and restore `totals`.
pub fn revert_block<H: TransactionHandlers>(
    wallets: &mut WalletRepository,
    totals: &mut Totals,
    undo: &BlockUndo,
    milestones: &Milestones,
    handlers: &H,
) -> Result<(), RevertError> {
    let _ = (wallets, totals, undo, milestones, handlers);
    todo!()
}
