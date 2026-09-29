//! The pool's transactions in the reference's orders.
//!
//! - all: the sender mempools in the order they were created, each newest first;
//! - by sender: oldest first;
//! - from the lowest priority: each sender's newest-first list merged by fee, lowest first;
//! - from the highest priority: each sender's oldest-first list merged by fee, highest first.
//!
//! A merge takes, at each step, the head with the lowest (or highest) fee; on a tie, the head of
//! the sender mempool created first, as the reference's `IteratorMany` keeps the first minimum.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::{Transaction, TransactionId, TransactionType};
use heartwood_transactions::PoolQuery;

use crate::mempool::Mempool;
use crate::sender_mempool::SenderMempool;

/// The pool's transactions, read in the reference's orders.
#[derive(Debug, Clone, Copy)]
pub struct Query<'p> {
    mempool: &'p Mempool,
}

impl<'p> Query<'p> {
    /// A query over `mempool`.
    pub fn new(mempool: &'p Mempool) -> Query<'p> {
        Query { mempool }
    }

    /// Every transaction: the sender mempools in creation order, each newest first.
    pub fn all(&self) -> Vec<&'p Transaction> {
        self.mempool
            .senders()
            .flat_map(SenderMempool::from_latest)
            .collect()
    }

    /// The transactions of `sender`, oldest first.
    pub fn by_sender(&self, sender: &Address) -> Vec<&'p Transaction> {
        self.mempool
            .sender(sender)
            .map(|mempool| mempool.from_earliest().collect())
            .unwrap_or_default()
    }

    /// Every transaction, from the lowest priority.
    pub fn from_lowest_priority(&self) -> Vec<&'p Transaction> {
        let lists = self.mempool.senders().map(|sender| sender.shared().rev());
        let merged = merge(lists, |fee, sender| Reverse((fee, sender)));
        merged.into_iter().map(Arc::as_ref).collect()
    }

    /// Every transaction, from the highest priority.
    pub fn from_highest_priority(&self) -> Vec<&'p Transaction> {
        let merged = self.shared_from_highest_priority();
        merged.into_iter().map(Arc::as_ref).collect()
    }

    /// [`Query::from_highest_priority`], as the pool holds the transactions (shared): what a
    /// published view of the pool lists.
    pub fn shared_from_highest_priority(&self) -> Vec<&'p Arc<Transaction>> {
        let lists = self.mempool.senders().map(SenderMempool::shared);
        merge(lists, |fee, sender| (fee, Reverse(sender)))
    }

    /// The transaction of the lowest priority, the first that
    /// [`Query::from_lowest_priority`] gives: the newest transaction of each sender with the
    /// lowest fee, of the earliest sender mempool on a tie.
    pub fn lowest_priority(&self) -> Option<&'p Transaction> {
        let mut lowest: Option<&'p Transaction> = None;
        for transaction in self
            .mempool
            .senders()
            .filter_map(|sender| sender.from_latest().next())
        {
            if lowest.is_none_or(|lowest| transaction.fee() < lowest.fee()) {
                lowest = Some(transaction);
            }
        }
        lowest
    }
}

/// Merge the lists of the sender mempools, in creation order, as the reference's `IteratorMany`
/// does: each step takes the head whose `key` (of its fee and its list's position) is the
/// greatest, and moves that list on.
fn merge<'p, I, K>(
    lists: impl Iterator<Item = I>,
    key: impl Fn(u64, usize) -> K,
) -> Vec<&'p Arc<Transaction>>
where
    I: Iterator<Item = &'p Arc<Transaction>>,
    K: Ord,
{
    let mut lists: Vec<I> = lists.collect();
    let mut heads: Vec<Option<&'p Arc<Transaction>>> = Vec::with_capacity(lists.len());
    let mut heap = BinaryHeap::with_capacity(lists.len());
    for (position, list) in lists.iter_mut().enumerate() {
        let head = list.next();
        if let Some(transaction) = head {
            heap.push((key(transaction.fee(), position), position));
        }
        heads.push(head);
    }
    let mut merged = Vec::new();
    while let Some((_, position)) = heap.pop() {
        let (Some(head), Some(list)) = (heads.get_mut(position), lists.get_mut(position)) else {
            continue;
        };
        if let Some(transaction) = head.take() {
            merged.push(transaction);
        }
        *head = list.next();
        if let Some(transaction) = *head {
            heap.push((key(transaction.fee(), position), position));
        }
    }
    merged
}

impl PoolQuery for Query<'_> {
    fn has_sender_kind(&self, sender: &Address, kind: TransactionType) -> bool {
        self.by_sender(sender)
            .iter()
            .any(|transaction| transaction.kind() == kind)
    }

    fn has_pending_username(&self, username: &str) -> bool {
        self.all()
            .iter()
            .any(|transaction| is_registration_of(transaction, username))
    }
}

/// The pool's transactions without the ones about to be evicted, as the pool-entry checks of a
/// new transaction see them once the eviction has happened.
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryExcept<'p> {
    /// The pool.
    pub(crate) query: Query<'p>,
    /// The transactions that are not counted.
    pub(crate) excluded: &'p [TransactionId],
}

impl PoolQuery for QueryExcept<'_> {
    fn has_sender_kind(&self, sender: &Address, kind: TransactionType) -> bool {
        self.query.by_sender(sender).iter().any(|transaction| {
            transaction.kind() == kind && !self.excluded.contains(transaction.id())
        })
    }

    fn has_pending_username(&self, username: &str) -> bool {
        self.query.all().iter().any(|transaction| {
            is_registration_of(transaction, username) && !self.excluded.contains(transaction.id())
        })
    }
}

/// Whether `transaction` registers the delegate name `username`.
fn is_registration_of(transaction: &Transaction, username: &str) -> bool {
    matches!(&transaction.data().asset, Asset::DelegateRegistration { username: name } if name == username)
}
