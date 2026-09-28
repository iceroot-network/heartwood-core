//! The published view of the pool: what the API reads between the node's operations on the pool.
//!
//! The node publishes a [`PoolView`] after each operation that changes the pool or the online
//! record: its transactions from the highest priority, the online record's entries, and the
//! options the API shows. The view holds the pool's own transactions, which the pool holds
//! shared, so a publish orders the pool (a merge of the senders' lists) and copies none of them.

use std::sync::Arc;

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::{Transaction, TransactionId};

use crate::online::{DelegateVersions, OnlineEntry};
use crate::options::{Options, PoolFees};
use crate::service::Pool;

/// The pool's options as the API shows them (`/node/configuration`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolViewOptions {
    /// Whether the pool runs.
    pub enabled: bool,
    /// When the pool holds this many transactions, a new one must evict the lowest.
    pub max_transactions_in_pool: usize,
    /// The most transactions a sender may have in the pool.
    pub max_transactions_per_sender: usize,
    /// The senders without a per-sender limit.
    pub allowed_senders: Vec<Address>,
    /// The most transactions one request may carry.
    pub max_transactions_per_request: usize,
    /// How many blocks a transaction may stay in the pool.
    pub max_transaction_age: u32,
    /// The longest transaction the pool takes, in bytes.
    pub max_transaction_bytes: usize,
    /// The pool's own fee rules.
    pub dynamic_fees: PoolFees,
}

impl PoolViewOptions {
    /// The shown part of `options`.
    pub fn of(options: &Options) -> PoolViewOptions {
        PoolViewOptions {
            enabled: options.enabled,
            max_transactions_in_pool: options.max_transactions_in_pool,
            max_transactions_per_sender: options.max_transactions_per_sender,
            allowed_senders: options.allowed_senders.clone(),
            max_transactions_per_request: options.max_transactions_per_request,
            max_transaction_age: options.max_transaction_age,
            max_transaction_bytes: options.max_transaction_bytes,
            dynamic_fees: options.dynamic_fees.clone(),
        }
    }
}

/// The pool and the online record, as the node publishes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolView {
    /// The pool's transactions, from the highest priority, as the reference's
    /// `getFromHighestPriority` lists them.
    pub by_priority: Vec<Arc<Transaction>>,
    /// The online record's entries, by public key.
    pub online: Vec<OnlineEntry>,
    /// The pool's options.
    pub options: PoolViewOptions,
}

impl PoolView {
    /// The number of transactions in the pool.
    pub fn len(&self) -> usize {
        self.by_priority.len()
    }

    /// Whether the pool holds no transaction.
    pub fn is_empty(&self) -> bool {
        self.by_priority.is_empty()
    }

    /// The pool's transaction with id `id`, if it holds one.
    pub fn get(&self, id: &TransactionId) -> Option<&Arc<Transaction>> {
        self.by_priority
            .iter()
            .find(|transaction| transaction.id() == id)
    }

    /// The version that the delegate with `public_key` last announced, if the online record has
    /// an entry for it.
    pub fn version(&self, public_key: &PublicKeyBytes) -> Option<&str> {
        self.online
            .binary_search_by(|entry| entry.public_key.cmp(public_key))
            .ok()
            .and_then(|index| self.online.get(index))
            .map(|entry| entry.version.as_str())
    }
}

impl Pool {
    /// The view of the pool, with the online record `online`, to publish. It shares the pool's
    /// transactions.
    pub fn view(&self, online: &DelegateVersions) -> PoolView {
        let shared = self.query().shared_from_highest_priority();
        PoolView {
            by_priority: shared.into_iter().map(Arc::clone).collect(),
            online: online.entries(),
            options: PoolViewOptions::of(self.options()),
        }
    }

    /// The view after the last one published, `previous`: [`Pool::view`], which already shares
    /// with `previous` every transaction the pool still holds, since both hold the pool's own.
    pub fn view_after(&self, online: &DelegateVersions, previous: &PoolView) -> PoolView {
        let _ = previous;
        self.view(online)
    }
}
