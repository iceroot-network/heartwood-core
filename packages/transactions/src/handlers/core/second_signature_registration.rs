//! 1/1 second-signature registration.
//!
//! Checks: the sender has no second key yet; the key is not the sender's own key. Apply: set the
//! second key. Revert: forget it. Pool entry: one registration per sender in the pool.

use heartwood_crypto::Transaction;
use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_state::ApplyContext;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;

use crate::errors::{PoolError, TransactionError};
use crate::handlers::transaction::{self, Sender};
use crate::pool_query::{OnlineDelegates, PoolQuery};

/// Run the type's checks, then the base checks, without changing `wallets`. `second` is the key
/// to register.
pub fn check(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    second: &PublicKeyBytes,
) -> Result<(), TransactionError> {
    let sender = Sender::of(ctx, wallets, transaction);
    if sender.wallet.second_public_key().is_some() {
        return Err(TransactionError::SecondSignatureAlreadyRegistered);
    }
    if sender.public_key(transaction).as_bytes() == second.as_bytes() {
        return Err(TransactionError::PublicKeyAlreadyAssociatedWithWallet);
    }
    transaction::check_base(ctx, wallets, transaction, &sender)
}

/// Check, then apply `transaction`; returns what its revert needs.
pub fn apply(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    second: &PublicKeyBytes,
) -> Result<TxUndo, TransactionError> {
    check(ctx, wallets, transaction, second)?;
    let address = transaction::apply_to_sender(ctx, wallets, transaction)?;
    wallets
        .get_or_create(&address)
        .set_second_public_key(Some(*second));
    Ok(TxUndo::None)
}

/// Revert `transaction` with the undo record its apply returned.
pub fn revert(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    undo: &TxUndo,
) -> Result<(), TransactionError> {
    let _ = undo;
    let address = transaction::revert_for_sender(ctx, wallets, transaction)?;
    wallets.get_or_create(&address).set_second_public_key(None);
    Ok(())
}

/// The genesis path: bind the sender's key and set the second key. Returns 0: the sender owes
/// nothing beyond the amount and the fee, which the caller takes.
pub fn bootstrap(
    ctx: &ApplyContext<'_>,
    wallets: &mut WalletRepository,
    transaction: &Transaction,
    second: &PublicKeyBytes,
) -> Result<u64, TransactionError> {
    let address = transaction::bind_sender_key(ctx, wallets, transaction);
    wallets
        .get_or_create(&address)
        .set_second_public_key(Some(*second));
    Ok(0)
}

/// Refuse `transaction` at pool entry when the sender already has a second-signature
/// registration in the pool: `ERR_PENDING`.
pub fn throw_if_cannot_enter_pool<P: PoolQuery, O: OnlineDelegates>(
    ctx: &ApplyContext<'_>,
    wallets: &WalletRepository,
    transaction: &Transaction,
    pool: &P,
    online: &O,
) -> Result<(), PoolError> {
    let _ = (wallets, online);
    let sender = transaction.data().sender_address(ctx.params);
    if pool.has_sender_kind(&sender, transaction.kind()) {
        return Err(PoolError::SenderPending {
            sender,
            kind: transaction.kind(),
        });
    }
    Ok(())
}
