//! 1/6 transfer: one or more items to recipients.
//!
//! Checks: the balance must cover the items and the fee. Apply: take the items' sum from the
//! sender and credit each item to its recipient, a repeated recipient once per item. Revert: the
//! inverse. At genesis the items are credited, and the sender owes their sum unless the transfer
//! is an issuance record.

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_state::ApplyContext;
use heartwood_state::errors::WalletError;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;

use crate::errors::TransactionError;
use crate::handlers::transaction::{self, Sender, Snapshot};

/// The sum of the items, which cannot overflow `u128`.
fn total(items: &[TransferItem]) -> u128 {
    items
        .iter()
        .fold(0_u128, |sum, item| sum.saturating_add(item.amount.into()))
}

/// Run the type's checks, then the base checks, without changing `wallets`.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    items: &[TransferItem],
) -> Result<(), TransactionError> {
    let sender = Sender::of(ctx, wallets, transaction);
    let needed = total(items).saturating_add(transaction.fee().into());
    let balance = sender.wallet.balance();
    if u128::from(balance) < needed {
        return Err(TransactionError::InsufficientBalance { needed, balance });
    }
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    items: &[TransferItem],
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction, items)?;
    let sender = transaction.data().sender_address(ctx.params);
    let touched = std::iter::once(&sender).chain(items.iter().map(|item| &item.recipient));
    Snapshot::take(wallets, touched).guard(wallets, |wallets| {
        let address = transaction::apply_to_sender(ctx, wallets, transaction)?;
        // The check made the balance cover the items and the fee.
        let sum =
            u64::try_from(total(items)).map_err(|_| WalletError::NegativeBalance { address })?;
        wallets.get_or_create(&address).debit(sum)?;
        for item in items {
            wallets.get_or_create(&item.recipient).credit(item.amount)?;
        }
        Ok(TxUndo::None)
    })
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    items: &[TransferItem],
    undo: &TxUndo,
) -> Result<(), TransactionError> {
    let _ = undo;
    let sender = transaction.data().sender_address(ctx.params);
    let touched = std::iter::once(&sender).chain(items.iter().map(|item| &item.recipient));
    Snapshot::take(wallets, touched).guard(wallets, |wallets| {
        let address = transaction::revert_for_sender(ctx, wallets, transaction)?;
        let sum =
            u64::try_from(total(items)).map_err(|_| WalletError::BalanceOverflow { address })?;
        wallets.get_or_create(&address).credit(sum)?;
        for item in items {
            wallets.get_or_create(&item.recipient).debit(item.amount)?;
        }
        Ok(())
    })
}

/// The genesis path: bind the sender's key and credit the items. Returns what the sender owes
/// for the items: their sum, or 0 when the transfer is an issuance record.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    items: &[TransferItem],
    issuance: bool,
) -> Result<u64, TransactionError> {
    let sender = transaction.data().sender_address(ctx.params);
    let owed = if issuance {
        0
    } else {
        u64::try_from(total(items)).map_err(|_| WalletError::NegativeBalance { address: sender })?
    };
    let touched = std::iter::once(&sender).chain(items.iter().map(|item| &item.recipient));
    Snapshot::take(wallets, touched).guard(wallets, |wallets| {
        transaction::bind_sender_key(ctx, wallets, transaction);
        for item in items {
            wallets.get_or_create(&item.recipient).credit(item.amount)?;
        }
        Ok(owed)
    })
}
