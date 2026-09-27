//! The mempool: one [`SenderMempool`] per sender, in the order the senders' mempools were
//! created. A sender mempool that becomes empty is dropped, and a new one for the same sender
//! goes last.

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionId};
use indexmap::IndexMap;
use indexmap::map::Values;

use crate::errors::Refusal;
use crate::options::Options;
use crate::sender_mempool::SenderMempool;
use crate::service::PoolContext;

/// Every sender's pending transactions.
#[derive(Debug, Clone, Default)]
pub struct Mempool {
    senders: IndexMap<Address, SenderMempool>,
}

impl Mempool {
    /// An empty mempool.
    pub fn new() -> Mempool {
        Mempool::default()
    }

    /// The number of transactions.
    pub fn len(&self) -> usize {
        self.senders.values().map(SenderMempool::len).sum()
    }

    /// Whether the mempool holds no transaction.
    pub fn is_empty(&self) -> bool {
        self.senders.values().all(SenderMempool::is_empty)
    }

    /// Whether `address` has a sender mempool.
    pub fn has_sender(&self, address: &Address) -> bool {
        self.senders.contains_key(address)
    }

    /// The sender mempool of `address`.
    pub fn sender(&self, address: &Address) -> Option<&SenderMempool> {
        self.senders.get(address)
    }

    /// Every sender mempool, in the order they were created.
    pub fn senders(&self) -> Values<'_, Address, SenderMempool> {
        self.senders.values()
    }

    /// Add `transaction` to its sender's mempool, creating it with a fresh view of `ctx.wallets`
    /// when the sender has none: the per-sender limit, then the sender state's checks and apply.
    /// A sender mempool left empty by a refusal is dropped.
    pub fn add_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        transaction: Transaction,
    ) -> Result<(), Refusal> {
        let _ = (ctx, options, transaction);
        todo!("add to the sender mempool")
    }

    /// Remove the transaction `id` of `sender` and the sender's later transactions (see
    /// [`SenderMempool::remove_transaction`]).
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        sender: &Address,
        id: &TransactionId,
    ) -> Vec<Transaction> {
        let _ = (ctx, sender, id);
        todo!("remove from the sender mempool")
    }

    /// Remove the forged transaction `id` of `sender` and the sender's earlier transactions (see
    /// [`SenderMempool::remove_forged_transaction`]).
    pub fn remove_forged_transaction(
        &mut self,
        sender: &Address,
        id: &TransactionId,
    ) -> Vec<Transaction> {
        let _ = (sender, id);
        todo!("remove forged from the sender mempool")
    }

    /// Drop every sender mempool.
    pub fn flush(&mut self) {
        self.senders.clear();
    }
}
