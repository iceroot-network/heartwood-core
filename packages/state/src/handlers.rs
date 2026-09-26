//! The seam between the state and the transaction handlers.
//!
//! [`TransactionHandlers`] is how the state applies, reverts and bootstraps a transaction
//! without depending on `heartwood-transactions`, which depends on this crate. It has exactly one
//! implementation, `heartwood_transactions::Handlers`, chosen at compile time: there is no
//! registration and no trait object, so no plugin can replace a rule.

use heartwood_crypto::{Params, Transaction};
use thiserror::Error;

use crate::errors::WalletError;
use crate::undo::TxUndo;
use crate::wallets::WalletRepository;

/// The heights and rules a transaction is applied, reverted or bootstrapped with.
///
/// For block `H`: `height` is `H`, `params` the milestone at `H`, and `last_block_height` is
/// `H − 1`. For the pool, `height` is the tip plus one and `last_block_height` the tip.
#[derive(Debug, Clone, Copy)]
pub struct ApplyContext<'a> {
    /// The height whose rules apply.
    pub height: u32,
    /// The merged milestone at `height`.
    pub params: &'a Params,
    /// The height of the last block before the transaction's block.
    pub last_block_height: u32,
}

/// A transaction was refused. `class` is the reference's error class for the refusal, which the
/// differential tests compare; `detail` is for people.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{class}: {detail}")]
pub struct TransactionRejected {
    /// The reference's error class, such as `InsufficientBalanceError`.
    pub class: &'static str,
    /// What was refused.
    pub detail: String,
}

/// A wallet value that left its range, as a refusal: the reference's failed `assert` on a negative
/// balance is an `AssertionError`, and its other throws in the same places are plain `Error`s.
impl From<WalletError> for TransactionRejected {
    fn from(error: WalletError) -> TransactionRejected {
        let class = match error {
            WalletError::NegativeBalance { .. } => "AssertionError",
            _ => "Error",
        };
        TransactionRejected {
            class,
            detail: error.to_string(),
        }
    }
}

/// The transaction rules, as the state calls them.
///
/// Every method leaves the wallets exactly as they were when it returns an error: a handler runs
/// all of its checks before it changes a wallet.
pub trait TransactionHandlers {
    /// Build the state of a genesis transaction, as the reference's start-up does: bind the
    /// sender's key, then the type's effect (credit transfer items, set the second key, the
    /// delegate, the resignation or the votes). The sender's nonce and its debit of amount and
    /// fee are left to the caller. Returns what the sender still owes for the transaction's
    /// items: the sum of a transfer's items, or 0 for an issuance record (`issuance`) and every
    /// other type.
    fn bootstrap(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
        issuance: bool,
    ) -> Result<u64, TransactionRejected>;

    /// Check and apply `transaction`, returning what its revert needs.
    fn apply(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
    ) -> Result<TxUndo, TransactionRejected>;

    /// Revert `transaction`, the last one applied, with the undo record its apply returned.
    fn revert(
        &self,
        ctx: &ApplyContext<'_>,
        wallets: &mut WalletRepository,
        transaction: &Transaction,
        undo: &TxUndo,
    ) -> Result<(), TransactionRejected>;
}
