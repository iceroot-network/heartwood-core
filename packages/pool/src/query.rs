//! The pool's transactions in the reference's orders.
//!
//! - all: the sender mempools in the order they were created, each newest first;
//! - by sender: oldest first;
//! - from the lowest priority: each sender's newest-first list merged by fee, lowest first;
//! - from the highest priority: each sender's oldest-first list merged by fee, highest first.
//!
//! A merge takes, at each step, the head with the lowest (or highest) fee; on a tie, the head of
//! the sender mempool created first, as the reference's `IteratorMany` keeps the first minimum.

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Transaction, TransactionType};
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
        todo!("merge by fee, lowest first")
    }

    /// Every transaction, from the highest priority.
    pub fn from_highest_priority(&self) -> Vec<&'p Transaction> {
        todo!("merge by fee, highest first")
    }

    /// The transaction of the lowest priority, the first that
    /// [`Query::from_lowest_priority`] gives.
    pub fn lowest_priority(&self) -> Option<&'p Transaction> {
        todo!("the lowest priority")
    }
}

impl PoolQuery for Query<'_> {
    fn has_sender_kind(&self, sender: &Address, kind: TransactionType) -> bool {
        let _ = (sender, kind);
        todo!("a pending transaction of this kind")
    }

    fn has_pending_username(&self, username: &str) -> bool {
        let _ = username;
        todo!("a pending registration of this name")
    }
}
