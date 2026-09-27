//! The pool: add, re-add, remove, remove forged, clean up and flush, as the reference's pool
//! service does them.
//!
//! A synced node calls it in this order: after a block is applied, `remove_forged_transaction`
//! for each of the block's transactions, then `clean_up`, then `readd(&[], true)`, when its block
//! queue drains; after a revert, `readd` with the reverted blocks' transactions.

use std::path::Path;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Milestones, Network, Params, Transaction, TransactionId};
use heartwood_state::ApplyContext;
use heartwood_state::wallets::{Wallet, WalletRepository};

use crate::errors::{AddError, StorageError};
use crate::mempool::Mempool;
use crate::online::DelegateVersions;
use crate::options::Options;
use crate::query::Query;
use crate::storage::{Storage, StoredTransaction};
use crate::verifier::Checked;

/// What every operation of the pool reads besides the pool: the state's wallets and tip, the
/// rules, and the online record.
#[derive(Debug, Clone, Copy)]
pub struct PoolContext<'a> {
    /// The wallets of the state at the tip. A sender mempool's view starts as a copy of them.
    pub wallets: &'a WalletRepository,
    /// The height of the tip.
    pub tip: u32,
    /// The milestones.
    pub milestones: &'a Milestones,
    /// The network.
    pub network: &'a Network,
    /// Which delegates operate a node.
    pub online: &'a DelegateVersions,
}

impl<'a> PoolContext<'a> {
    /// The height whose rules the pool applies: the tip plus one.
    pub fn height(&self) -> u32 {
        self.tip.saturating_add(1)
    }

    /// The rules of [`PoolContext::height`].
    pub fn params(&self) -> &'a Params {
        self.milestones.at(self.height())
    }

    /// The context in which the handlers apply and revert a pool transaction: the rules of the
    /// tip plus one, with the tip as the last block.
    pub fn apply_context(&self) -> ApplyContext<'a> {
        ApplyContext {
            height: self.height(),
            params: self.params(),
            last_block_height: self.tip,
        }
    }
}

/// What a re-add did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readded {
    /// The previously forged transactions added back, in the order they were given.
    pub forged: Vec<TransactionId>,
    /// The previously forged transactions that could not be added back, with why.
    pub forged_failed: Vec<(TransactionId, String)>,
    /// The stored transactions added back, in storage order.
    pub stored: Vec<TransactionId>,
    /// The stored transactions removed because they were older than `maxTransactionAge`.
    pub expired: Vec<TransactionId>,
    /// The stored transactions removed because they could not be added back, with why.
    pub failed: Vec<(TransactionId, String)>,
}

/// What a clean-up removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    /// The transactions older than `maxTransactionAge`, with their senders' later transactions,
    /// in the order they were removed.
    pub old: Vec<Transaction>,
    /// The lowest-priority transactions removed while the pool held more than
    /// `maxTransactionsInPool`, with their senders' later transactions.
    pub lowest_priority: Vec<Transaction>,
}

/// The transaction pool: its options, its storage and its sender mempools.
#[derive(Debug)]
pub struct Pool {
    options: Options,
    storage: Storage,
    mempool: Mempool,
}

impl Pool {
    /// Open the pool with `options`, its storage at `options.storage`. `clear_marker`, when it
    /// names a file that exists, empties the storage first (see [`Storage::open`]). The pool
    /// starts with no transaction in memory; the node's first `readd` adds the stored ones.
    pub fn open(options: Options, clear_marker: Option<&Path>) -> Result<Pool, StorageError> {
        let _ = (options, clear_marker);
        todo!("open the storage")
    }

    /// The pool's options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Add `transaction`, which the processor's first loop decoded: the duplicate check, the fee
    /// rule, room in the pool (evicting the lowest priority when the fee is above it), then the
    /// sender mempool and the sender's view; an admitted transaction is stored.
    pub fn add_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        transaction: Checked,
    ) -> Result<(), AddError> {
        let _ = (ctx, transaction);
        todo!("admit a transaction")
    }

    /// Empty the mempool (not the storage) and add back first `previously_forged` (the
    /// transactions of reverted blocks, which are then stored), then every stored transaction
    /// that is neither among them nor too old. Re-adds skip the duplicate and fee steps; the
    /// handlers' own checks apply. `recheck` is the reference's flag of a re-add after a block,
    /// which changes only what is logged.
    pub fn readd(
        &mut self,
        ctx: &PoolContext<'_>,
        previously_forged: &[Transaction],
        recheck: bool,
    ) -> Result<Readded, StorageError> {
        let _ = (ctx, previously_forged, recheck);
        todo!("re-add the stored transactions")
    }

    /// Remove the transaction `id` of `sender` and the sender's later transactions, reverting
    /// them on the sender's view, newest first; returns what was removed.
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<Vec<Transaction>, StorageError> {
        let _ = (ctx, sender, id);
        todo!("remove a transaction")
    }

    /// Remove a forged transaction: the sender's transactions up to and including `id`, without
    /// a revert; returns what was removed.
    pub fn remove_forged_transaction(
        &mut self,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<Vec<Transaction>, StorageError> {
        let _ = (sender, id);
        todo!("remove a forged transaction")
    }

    /// Remove the transactions older than `maxTransactionAge`, then the lowest-priority ones
    /// while the pool holds more than `maxTransactionsInPool`.
    pub fn clean_up(&mut self, ctx: &PoolContext<'_>) -> Result<Removed, StorageError> {
        let _ = ctx;
        todo!("clean up")
    }

    /// Empty the mempool and the storage.
    pub fn flush(&mut self) -> Result<(), StorageError> {
        todo!("flush")
    }

    /// The number of transactions in the pool.
    pub fn size(&self) -> usize {
        self.mempool.len()
    }

    /// The pool's transactions, in the reference's orders.
    pub fn query(&self) -> Query<'_> {
        Query::new(&self.mempool)
    }

    /// The sender mempools.
    pub fn mempool(&self) -> &Mempool {
        &self.mempool
    }

    /// The wallet at `address` in the view of its sender mempool, if it has one.
    pub fn pool_wallet(&self, address: &Address) -> Option<&Wallet> {
        let _ = address;
        todo!("the sender's view of its wallet")
    }

    /// Every stored row, in storage order.
    pub fn stored(&self) -> Result<Vec<StoredTransaction>, StorageError> {
        self.storage.all()
    }
}
