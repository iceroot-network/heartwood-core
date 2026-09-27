//! A sender's view of the wallets, and the checks a transaction passes on it: the size limit,
//! the network byte, the signature, the view's consistency, then the pool-entry checks and the
//! handler's apply, whose refusals become `ERR_APPLY`.

use heartwood_crypto::Transaction;
use heartwood_state::errors::TransactionFailure;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;
use heartwood_transactions::{PoolQuery, TransactionError};

use crate::errors::Refusal;
use crate::options::Options;
use crate::service::PoolContext;

/// A sender's view of the wallets.
#[derive(Debug, Clone)]
pub struct SenderState {
    wallets: WalletRepository,
    corrupt: bool,
}

impl SenderState {
    /// A view that starts as a copy of `wallets`.
    pub fn new(wallets: &WalletRepository) -> SenderState {
        SenderState {
            wallets: wallets.clone(),
            corrupt: false,
        }
    }

    /// The view's wallets.
    pub fn wallets(&self) -> &WalletRepository {
        &self.wallets
    }

    /// Whether a failed revert left the view inconsistent; every later transaction of the sender
    /// is then refused with `ERR_RETRY`.
    pub fn is_corrupt(&self) -> bool {
        self.corrupt
    }

    /// Check `transaction` and apply it to the view, in the reference's order: `ERR_TOO_LARGE`,
    /// `ERR_WRONG_NETWORK`, `ERR_BAD_DATA` for a signature that did not verify, `ERR_RETRY`, then
    /// the pool-entry checks against `pending` and the handler's apply, any refusal of which is
    /// `ERR_APPLY`. Returns what reverting it needs.
    pub fn apply<P: PoolQuery>(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        pending: &P,
        transaction: &Transaction,
    ) -> Result<TxUndo, Refusal> {
        let _ = (ctx, options, pending, transaction);
        todo!("the sender state's checks and apply")
    }

    /// Revert `transaction`, the last one applied to the view, with its undo record. A failure
    /// marks the view inconsistent.
    pub fn revert(
        &mut self,
        ctx: &PoolContext<'_>,
        transaction: &Transaction,
        undo: &TxUndo,
    ) -> Result<(), TransactionFailure<TransactionError>> {
        let _ = (ctx, transaction, undo);
        todo!("revert on the view")
    }
}
