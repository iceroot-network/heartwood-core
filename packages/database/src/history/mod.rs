//! The queries of the served routes, on a [`Reader`]: blocks by id or height, the transactions of
//! the criteria with the reference's orders and pages, a transaction by id, and a round's rows.
//!
//! A listing counts every match (`totalCount`) and returns the page; ties of the requested order
//! are broken by the unique key, so a page is the same on every call. A statement interrupted
//! after [`crate::READ_TIMEOUT`] is a [`DatabaseError`], which the API answers with a logged 500.

use heartwood_crypto::{BlockId, TransactionId};

use crate::blocks::BlockRow;
use crate::errors::DatabaseError;
use crate::filters::TransactionFilter;
use crate::reader::Reader;
use crate::rounds::RoundRow;
use crate::transactions::TransactionRow;

/// A block by its id or its height, as the API's `{id}` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockSelector {
    /// A full block id.
    Id(BlockId),
    /// A height.
    Height(u32),
}

/// A direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Lowest first.
    Asc,
    /// Highest first.
    Desc,
}

/// One key of an order, by the reference's property name (`timestamp`, `sequence`, `amount`,
/// ...), and its direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// The property.
    pub property: String,
    /// The direction.
    pub direction: Direction,
}

/// A page: how many matches to skip, and how many to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// The matches skipped.
    pub offset: u64,
    /// The most matches returned.
    pub limit: u64,
}

/// A page of matches and their total count, as the reference's `ResultsPage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing<T> {
    /// The page's matches, in order.
    pub rows: Vec<T>,
    /// Every match's count.
    pub total_count: u64,
    /// Whether the count is an estimate (`CORE_API_ESTIMATED_TOTAL_COUNT`); never here.
    pub total_count_is_estimate: bool,
}

/// A stored transaction, with its block's height and timestamp, as the transaction routes read
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord {
    /// The transaction's row, with its recipients.
    pub row: TransactionRow,
}

impl Reader<'_> {
    /// The rows of the blocks at `heights` that the store holds, in height order.
    pub fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockRow>, DatabaseError> {
        let _ = (&self.connection, heights);
        todo!()
    }

    /// The row of the block `selector` names, if the store holds it.
    pub fn block(&self, selector: BlockSelector) -> Result<Option<BlockRow>, DatabaseError> {
        let _ = (&self.connection, selector);
        todo!()
    }

    /// The page `page` of the transactions that `filter` matches, in the order `order` (the
    /// unique key breaks ties).
    pub fn transactions(
        &self,
        filter: &TransactionFilter,
        order: &[Order],
        page: Page,
    ) -> Result<Listing<TransactionRecord>, DatabaseError> {
        let _ = (&self.connection, filter, order, page);
        todo!()
    }

    /// The transaction with id `id`, if the store holds it.
    pub fn transaction(
        &self,
        id: &TransactionId,
    ) -> Result<Option<TransactionRecord>, DatabaseError> {
        let _ = (&self.connection, id);
        todo!()
    }

    /// The rows of round `round`, in the reference's `getRound` order.
    pub fn round(&self, round: u64) -> Result<Vec<RoundRow>, DatabaseError> {
        let _ = (&self.connection, round);
        todo!()
    }
}
