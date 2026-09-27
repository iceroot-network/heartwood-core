//! A sender's view of the wallets, and the checks a transaction passes on it: the size limit,
//! the network byte, the signature, the view's consistency, then the pool-entry checks and the
//! handler's apply, whose refusals become `ERR_APPLY`.
//!
//! The view changes only through the handlers' apply and revert, as the reference's pool view
//! does: the vote balances of the delegates are the state's business, not the pool's. The
//! handlers' activation check of the reference sits between the network and the signature steps;
//! every type this implementation carries is always active, so it has no step here.

use heartwood_crypto::Transaction;
use heartwood_kernel::utils::pluralise::pluralise;
use heartwood_state::TransactionHandlers;
use heartwood_state::errors::TransactionFailure;
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::WalletRepository;
use heartwood_transactions::{Handlers, PoolQuery, TransactionError, check_pool_entry};

use crate::errors::{PoolCode, Refusal};
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

    /// A view with no wallet, which holds the place of a view taken out of its sender mempool.
    pub(crate) fn empty(network: u8) -> SenderState {
        SenderState {
            wallets: WalletRepository::new(network),
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
    ///
    /// The network step skips the network byte 0, which the reference reads as no network.
    pub fn apply<P: PoolQuery>(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        pending: &P,
        transaction: &Transaction,
    ) -> Result<TxUndo, Refusal> {
        let text = || transaction.display(ctx.network.pub_key_hash).to_string();
        if transaction.bytes().len() > options.max_transaction_bytes {
            let limit = i128::try_from(options.max_transaction_bytes).unwrap_or(i128::MAX);
            return Err(Refusal::new(
                PoolCode::TooLarge,
                format!(
                    "{} exceeds the size limit of {}",
                    text(),
                    pluralise("byte", limit, true)
                ),
            ));
        }
        let network = transaction.data().network;
        if network != 0 && network != ctx.network.pub_key_hash {
            return Err(Refusal::new(
                PoolCode::WrongNetwork,
                format!(
                    "{} network {network} doesn't match the node's network {}",
                    text(),
                    ctx.network.pub_key_hash
                ),
            ));
        }
        if !transaction.is_verified() {
            return Err(Refusal::new(
                PoolCode::BadData,
                format!("{} failed signature verification check", text()),
            ));
        }
        if self.corrupt {
            return Err(Refusal::new(
                PoolCode::Retry,
                format!("{} cannot be added to the pool, please retry", text()),
            ));
        }
        let apply_ctx = ctx.apply_context();
        let cannot_apply = |message: String| {
            Refusal::new(
                PoolCode::Apply,
                format!("{} cannot be applied: {message}", text()),
            )
        };
        let online = ctx.online.view(&self.wallets);
        check_pool_entry(&apply_ctx, &self.wallets, transaction, pending, &online)
            .map_err(|error| cannot_apply(error.to_string()))?;
        Handlers
            .apply(&apply_ctx, &mut self.wallets, transaction)
            .map_err(|error| cannot_apply(error.message(&ctx.network.symbol)))
    }

    /// Revert `transaction`, the last one applied to the view, with its undo record. A failure
    /// marks the view inconsistent.
    pub fn revert(
        &mut self,
        ctx: &PoolContext<'_>,
        transaction: &Transaction,
        undo: &TxUndo,
    ) -> Result<(), TransactionFailure<TransactionError>> {
        Handlers
            .revert(&ctx.apply_context(), &mut self.wallets, transaction, undo)
            .map_err(|error| {
                self.corrupt = true;
                TransactionFailure::Refused(error)
            })
    }
}
