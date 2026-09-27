//! One sender's pending transactions, in arrival order, with the sender's own view of the
//! wallets.
//!
//! As in the reference, each sender mempool has its own copy of the state's wallets, so a
//! sender's view never sees another sender's pending transactions: a pending transfer to a
//! wallet does not fund that wallet's own pool transactions.

use std::iter::Rev;
use std::slice::Iter;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionId};
use heartwood_state::undo::TxUndo;
use heartwood_state::wallets::{Wallet, WalletRepository};
use heartwood_transactions::PoolQuery;

use crate::errors::Refusal;
use crate::options::Options;
use crate::sender_state::SenderState;
use crate::service::PoolContext;

/// One sender's pending transactions.
#[derive(Debug, Clone)]
pub struct SenderMempool {
    address: Address,
    transactions: Vec<Transaction>,
    undo: Vec<TxUndo>,
    state: SenderState,
}

impl SenderMempool {
    /// An empty sender mempool for `address`, whose view starts as a copy of `wallets`.
    pub fn new(address: Address, wallets: &WalletRepository) -> SenderMempool {
        SenderMempool {
            address,
            transactions: Vec::new(),
            undo: Vec::new(),
            state: SenderState::new(wallets),
        }
    }

    /// The sender's address.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The number of transactions.
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Whether the sender mempool holds no transaction.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    /// The transactions, the oldest first.
    pub fn from_earliest(&self) -> Iter<'_, Transaction> {
        self.transactions.iter()
    }

    /// The transactions, the newest first.
    pub fn from_latest(&self) -> Rev<Iter<'_, Transaction>> {
        self.transactions.iter().rev()
    }

    /// The sender's state: its view of the wallets.
    pub fn state(&self) -> &SenderState {
        &self.state
    }

    /// The sender's wallet in its view.
    pub fn wallet(&self) -> Option<&Wallet> {
        self.state.wallets().find_by_address(&self.address)
    }

    /// Add `transaction`: refused with `ERR_EXCEEDS_MAX_COUNT` when the sender already has
    /// `maxTransactionsPerSender` transactions and is not an allowed sender, then checked and
    /// applied on the sender's view ([`SenderState::apply`]), with `pending` as the pool's pending
    /// transactions for the pool-entry checks.
    pub fn add_transaction<P: PoolQuery>(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        pending: &P,
        transaction: Transaction,
    ) -> Result<(), Refusal> {
        let _ = (ctx, options, pending, transaction, &self.undo);
        todo!("the per-sender limit, then the sender state")
    }

    /// Remove the transaction `id` and every later one, reverting them on the view newest first;
    /// when a revert fails, every transaction of the sender is removed and the view is marked
    /// inconsistent. Returns the removed transactions, newest first.
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        id: &TransactionId,
    ) -> Vec<Transaction> {
        let _ = (ctx, id);
        todo!("remove with cascade")
    }

    /// Remove the forged transaction `id` and every earlier one, without a revert (the state
    /// has applied them). Returns the removed transactions, oldest first; none when `id` is not
    /// here.
    pub fn remove_forged_transaction(&mut self, id: &TransactionId) -> Vec<Transaction> {
        let _ = id;
        todo!("remove forged")
    }
}
