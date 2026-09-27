//! The pool: add, re-add, remove, remove forged, clean up and flush, as the reference's pool
//! service does them.
//!
//! A synced node calls it in this order: after a block is applied, `remove_forged_transaction`
//! for each of the block's transactions, then `clean_up`, then `readd(&[], true)`, when its block
//! queue drains; after a revert, `readd` with the reverted blocks' transactions.

use std::path::Path;

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::utils::format_satoshi;
use heartwood_crypto::{Milestones, Network, Params, Transaction, TransactionId};
use heartwood_state::ApplyContext;
use heartwood_state::wallets::{Wallet, WalletRepository};

use crate::dynamic_fee_matcher::check_enter_pool;
use crate::errors::{AddError, PoolCode, Refusal, StorageError};
use crate::mempool::Mempool;
use crate::online::DelegateVersions;
use crate::options::Options;
use crate::query::{Query, QueryExcept};
use crate::sender_mempool::SenderMempool;
use crate::storage::{Storage, StoredTransaction, TABLE};
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
        let storage = Storage::open(&options.storage, clear_marker)?;
        Ok(Pool {
            options,
            storage,
            mempool: Mempool::new(),
        })
    }

    /// The pool's options.
    pub fn options(&self) -> &Options {
        &self.options
    }

    /// Add `transaction`, which the processor's first loop decoded: the duplicate check, the fee
    /// rule, room in the pool (evicting the lowest priority when the fee is above it), then the
    /// sender mempool and the sender's view; an admitted transaction is stored.
    ///
    /// Two steps differ from the reference, which writes the row first and evicts before it
    /// checks: the row is written only once the transaction is admitted, so a crash never leaves
    /// a row of an unchecked transaction; and in a full pool the transaction is first checked
    /// against the pool as the eviction would leave it, and a refusal evicts nothing.
    pub fn add_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        transaction: Checked,
    ) -> Result<(), AddError> {
        let transaction = match transaction {
            Checked::Decoded(transaction) => *transaction,
            // The reference's handler registry refuses the type in the fee step. Every removed
            // type is of type group 1, which the reference calls `Core`.
            Checked::Removed { kind, .. } => {
                return Err(Refusal::new(
                    PoolCode::Other,
                    format!("Transaction type Core/{kind} is deactivated"),
                )
                .into());
            }
        };
        if self.storage.has(transaction.id())? {
            return Err(Refusal::new(
                PoolCode::Duplicate,
                format!(
                    "{} is already in the pool",
                    transaction.display(ctx.network.pub_key_hash)
                ),
            )
            .into());
        }
        check_enter_pool(ctx, &self.options, &transaction)?;
        let row = stored_row(ctx, &transaction);
        self.add_to_mempool(ctx, transaction)?;
        self.storage.add(&row)?;
        Ok(())
    }

    /// Empty the mempool (not the storage) and add back first `previously_forged` (the
    /// transactions of reverted blocks, which are then stored), then every stored transaction
    /// that is neither among them nor too old. Re-adds skip the duplicate and fee steps; the
    /// handlers' own checks apply. (The reference's flag of a re-add after a block changes only
    /// what it logs, so there is none here.)
    ///
    /// Both are decoded as bytes this node checked before, as the reference's
    /// `fromBytesUnsafe` does. A previously forged transaction that is still stored stays in the
    /// mempool but counts as failed, as the reference's refused insert does; its row is then
    /// re-added like any other and fails on its nonce.
    pub fn readd(
        &mut self,
        ctx: &PoolContext<'_>,
        previously_forged: &[Transaction],
    ) -> Result<Readded, StorageError> {
        self.mempool.flush();
        let mut readded = Readded::default();
        let mut forged_stored = Vec::new();
        for forged in previously_forged {
            let id = *forged.id();
            match self.readd_one(ctx, forged.bytes(), &id)? {
                Err(why) => readded.forged_failed.push((id, why)),
                Ok(_) if self.storage.has(&id)? => {
                    let why = format!("UNIQUE constraint failed: {TABLE}.id");
                    readded.forged_failed.push((id, why));
                }
                Ok(row) => {
                    self.storage.add(&row)?;
                    forged_stored.push(id);
                    readded.forged.push(id);
                }
            }
        }
        let expired = self.expired_height(ctx);
        for row in self.storage.all()? {
            if forged_stored.contains(&row.id) {
                continue;
            }
            if i64::from(row.height) <= expired {
                self.storage.remove(&row.id)?;
                readded.expired.push(row.id);
                continue;
            }
            match self.readd_one(ctx, &row.serialised, &row.id)? {
                Ok(_) => readded.stored.push(row.id),
                Err(why) => {
                    self.storage.remove(&row.id)?;
                    readded.failed.push((row.id, why));
                }
            }
        }
        Ok(readded)
    }

    /// Decode `bytes` as trusted and add them to the mempool; returns the row to store, or why
    /// they were not added. A transaction whose id is not `id` is not added.
    fn readd_one(
        &mut self,
        ctx: &PoolContext<'_>,
        bytes: &[u8],
        id: &TransactionId,
    ) -> Result<Result<StoredTransaction, String>, StorageError> {
        let transaction = match Transaction::decode_trusted(bytes, ctx.params()) {
            Ok(transaction) if transaction.id() == id => transaction,
            Ok(transaction) => {
                return Ok(Err(format!("the bytes have the id {}", transaction.id())));
            }
            Err(error) => return Ok(Err(error.to_string())),
        };
        let row = stored_row(ctx, &transaction);
        match self.add_to_mempool(ctx, transaction) {
            Ok(()) => Ok(Ok(row)),
            Err(AddError::Refused(refusal)) => Ok(Err(refusal.message)),
            Err(AddError::Storage(error)) => Err(error),
        }
    }

    /// Remove the transaction `id` of `sender` and the sender's later transactions, reverting
    /// them on the sender's view, newest first; returns what was removed. Nothing happens when
    /// `id` is not stored.
    pub fn remove_transaction(
        &mut self,
        ctx: &PoolContext<'_>,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<Vec<Transaction>, StorageError> {
        if !self.storage.has(id)? {
            return Ok(Vec::new());
        }
        let removed = self.remove_cascade(ctx, sender, id)?;
        if !removed.iter().any(|transaction| transaction.id() == id) {
            self.storage.remove(id)?;
        }
        Ok(removed)
    }

    /// Remove a forged transaction: the sender's transactions up to and including `id`, without
    /// a revert; returns what was removed. Nothing happens when `id` is not stored.
    pub fn remove_forged_transaction(
        &mut self,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<Vec<Transaction>, StorageError> {
        if !self.storage.has(id)? {
            return Ok(Vec::new());
        }
        let removed = self.mempool.remove_forged_transaction(sender, id);
        for transaction in &removed {
            self.storage.remove(transaction.id())?;
        }
        if !removed.iter().any(|transaction| transaction.id() == id) {
            self.storage.remove(id)?;
        }
        Ok(removed)
    }

    /// Remove the transactions older than `maxTransactionAge`, then the lowest-priority ones
    /// while the pool holds more than `maxTransactionsInPool`.
    pub fn clean_up(&mut self, ctx: &PoolContext<'_>) -> Result<Removed, StorageError> {
        Ok(Removed {
            old: self.remove_old(ctx)?,
            lowest_priority: self
                .remove_lowest_above(ctx, self.options.max_transactions_in_pool)?,
        })
    }

    /// Empty the mempool and the storage.
    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.mempool.flush();
        self.storage.flush()
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
        self.mempool.sender(address).and_then(SenderMempool::wallet)
    }

    /// Every stored row, in storage order.
    pub fn stored(&self) -> Result<Vec<StoredTransaction>, StorageError> {
        self.storage.all()
    }

    /// Make room and add `transaction` to its sender mempool, as the reference's
    /// `addTransactionToMempool` does: in a full pool, the old transactions and those above the
    /// limit go first; then a fee at most the lowest's is `ERR_POOL_FULL`, and a higher one
    /// evicts the lowest-priority transaction with its sender's later ones.
    fn add_to_mempool(
        &mut self,
        ctx: &PoolContext<'_>,
        transaction: Transaction,
    ) -> Result<(), AddError> {
        let max = self.options.max_transactions_in_pool;
        if self.mempool.len() >= max {
            self.remove_old(ctx)?;
            self.remove_lowest_above(ctx, max)?;
        }
        if self.mempool.len() >= max {
            let Some((sender, id, fee)) = self.lowest(ctx) else {
                return Err(Refusal::new(PoolCode::Other, "Transaction not found").into());
            };
            if transaction.fee() <= fee {
                let symbol = &ctx.network.symbol;
                return Err(Refusal::new(
                    PoolCode::PoolFull,
                    format!(
                        "{} fee {} is lower than {} which is already in the pool",
                        transaction.display(ctx.network.pub_key_hash),
                        format_satoshi(transaction.fee().into(), symbol),
                        format_satoshi(fee.into(), symbol)
                    ),
                )
                .into());
            }
            self.check_before_eviction(ctx, &transaction, &sender, &id)?;
            self.remove_cascade(ctx, &sender, &id)?;
        }
        self.mempool
            .add_transaction(ctx, &self.options, transaction)
            .map_err(AddError::from)
    }

    /// Run the per-sender and sender-state steps for `transaction` on a copy of its sender
    /// mempool, against the pool as evicting the transaction `id` of `sender` (with that sender's
    /// later ones) would leave it; the pool is not changed.
    fn check_before_eviction(
        &self,
        ctx: &PoolContext<'_>,
        transaction: &Transaction,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<(), Refusal> {
        let address = transaction.data().sender_address(ctx.params());
        let ids = |removed: Vec<Transaction>| removed.iter().map(|pending| *pending.id()).collect();
        let (excluded, scratch): (Vec<TransactionId>, _) = if *sender == address {
            // The sender's own transactions go: its copy is the one they leave.
            let mut victim = self.mempool.sender(sender).cloned();
            let evicted = victim
                .as_mut()
                .map(|victim| ids(victim.remove_transaction(ctx, id)));
            (
                evicted.unwrap_or_default(),
                victim.filter(|victim| !victim.is_empty()),
            )
        } else {
            let evicted = self.mempool.sender(sender).map(|victim| {
                victim
                    .from_earliest()
                    .skip_while(|pending| pending.id() != id)
                    .map(|pending| *pending.id())
                    .collect()
            });
            (
                evicted.unwrap_or_default(),
                self.mempool.sender(&address).cloned(),
            )
        };
        let mut scratch = scratch.unwrap_or_else(|| SenderMempool::new(address, ctx.wallets));
        let pending = QueryExcept {
            query: self.query(),
            excluded: &excluded,
        };
        scratch.add_transaction(ctx, &self.options, &pending, transaction.clone())
    }

    /// The sender, id and fee of the lowest-priority transaction.
    fn lowest(&self, ctx: &PoolContext<'_>) -> Option<(Address, TransactionId, u64)> {
        self.query().lowest_priority().map(|lowest| {
            (
                lowest.data().sender_address(ctx.params()),
                *lowest.id(),
                lowest.fee(),
            )
        })
    }

    /// The height at or below which a stored transaction is too old.
    fn expired_height(&self, ctx: &PoolContext<'_>) -> i64 {
        i64::from(ctx.tip) - i64::from(self.options.max_transaction_age)
    }

    /// Remove the stored transactions at or below the expired height, the newest first, each
    /// with its sender's later ones.
    fn remove_old(&mut self, ctx: &PoolContext<'_>) -> Result<Vec<Transaction>, StorageError> {
        let mut removed = Vec::new();
        for (id, sender) in self.storage.old(self.expired_height(ctx))? {
            if let Ok(sender) = Address::from_base58(&sender) {
                removed.extend(self.remove_cascade(ctx, &sender, &id)?);
            }
        }
        Ok(removed)
    }

    /// Remove the lowest-priority transaction, with its sender's later ones, while the pool
    /// holds more than `max` transactions.
    fn remove_lowest_above(
        &mut self,
        ctx: &PoolContext<'_>,
        max: usize,
    ) -> Result<Vec<Transaction>, StorageError> {
        let mut removed = Vec::new();
        while self.mempool.len() > max {
            let Some((sender, id, _)) = self.lowest(ctx) else {
                break;
            };
            let cascade = self.remove_cascade(ctx, &sender, &id)?;
            if cascade.is_empty() {
                break;
            }
            removed.extend(cascade);
        }
        Ok(removed)
    }

    /// Remove the transaction `id` of `sender` and the sender's later ones from the mempool and
    /// the storage.
    fn remove_cascade(
        &mut self,
        ctx: &PoolContext<'_>,
        sender: &Address,
        id: &TransactionId,
    ) -> Result<Vec<Transaction>, StorageError> {
        let removed = self.mempool.remove_transaction(ctx, sender, id);
        for transaction in &removed {
            self.storage.remove(transaction.id())?;
        }
        Ok(removed)
    }
}

/// The row of an admitted transaction: the tip's height, and the recipients of a transfer
/// joined by commas.
fn stored_row(ctx: &PoolContext<'_>, transaction: &Transaction) -> StoredTransaction {
    let recipient_id = match &transaction.data().asset {
        Asset::Transfer { transfers } => transfers
            .iter()
            .map(|item| item.recipient.to_base58())
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    StoredTransaction {
        height: ctx.tip,
        id: *transaction.id(),
        recipient_id,
        sender_id: transaction.data().sender_address(ctx.params()).to_base58(),
        serialised: transaction.bytes().to_vec(),
    }
}
