//! The mempool: one [`SenderMempool`] per sender, in the order the senders' mempools were
//! created. A sender mempool that becomes empty is dropped, and a new one for the same sender
//! goes last.

use std::sync::Arc;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionId};
use indexmap::IndexMap;
use indexmap::map::Values;

use crate::errors::Refusal;
use crate::options::Options;
use crate::query::Query;
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
    /// when the sender has none: the per-sender limit, then the sender state's checks and apply,
    /// whose pool-entry checks read the whole pool. A sender mempool left empty by a refusal is
    /// dropped.
    pub fn add_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        options: &Options,
        transaction: Transaction,
    ) -> Result<(), Refusal> {
        let address = transaction.data().sender_address(ctx.params());
        let index = match self.senders.get_index_of(&address) {
            Some(index) => index,
            None => {
                self.senders
                    .insert_full(address, SenderMempool::new(address, ctx.wallets))
                    .0
            }
        };
        let result = self.add_at(index, ctx, options, transaction);
        if self
            .senders
            .get_index(index)
            .is_some_and(|(_, sender)| sender.is_empty())
        {
            self.senders.shift_remove_index(index);
        }
        result
    }

    /// Add `transaction` to the sender mempool at `index`. Its view is taken out while it checks
    /// the transaction, so that the pool-entry checks can read every sender's transactions.
    fn add_at(
        &mut self,
        index: usize,
        ctx: &PoolContext<'_>,
        options: &Options,
        transaction: Transaction,
    ) -> Result<(), Refusal> {
        let Some((_, sender)) = self.senders.get_index_mut(index) else {
            return Ok(());
        };
        sender.check_count(ctx, options, &transaction)?;
        let mut state = sender.take_state();
        let applied = state.apply(ctx, options, &Query::new(self), &transaction);
        let Some((_, sender)) = self.senders.get_index_mut(index) else {
            return Ok(());
        };
        sender.put_state(state);
        sender.push(transaction, applied?);
        Ok(())
    }

    /// Remove the transaction `id` of `sender` and the sender's later transactions (see
    /// [`SenderMempool::remove_transaction`]).
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        sender: &Address,
        id: &TransactionId,
    ) -> Vec<Arc<Transaction>> {
        let Some(mempool) = self.senders.get_mut(sender) else {
            return Vec::new();
        };
        let removed = mempool.remove_transaction(ctx, id);
        self.drop_if_empty(sender);
        removed
    }

    /// Remove the forged transaction `id` of `sender` and the sender's earlier transactions (see
    /// [`SenderMempool::remove_forged_transaction`]).
    pub fn remove_forged_transaction(
        &mut self,
        sender: &Address,
        id: &TransactionId,
    ) -> Vec<Arc<Transaction>> {
        let Some(mempool) = self.senders.get_mut(sender) else {
            return Vec::new();
        };
        let removed = mempool.remove_forged_transaction(id);
        self.drop_if_empty(sender);
        removed
    }

    /// Drop every sender mempool.
    pub fn flush(&mut self) {
        self.senders.clear();
    }

    /// Drop the sender mempool of `sender` when it holds no transaction, keeping the others in
    /// creation order.
    fn drop_if_empty(&mut self, sender: &Address) {
        if self
            .senders
            .get(sender)
            .is_some_and(SenderMempool::is_empty)
        {
            self.senders.shift_remove(sender);
        }
    }
}
