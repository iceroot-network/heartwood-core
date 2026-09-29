//! The block listing of `GET /blocks` and `GET /delegates/{id}/blocks`: the blocks of the block
//! criteria with the reference's orders and pages, and, for a transformed answer, the amounts of
//! each block's Core Transfer payments, which the resource adds to `forged.amount`.

use super::{Listing, Order, Page};
use crate::blocks::BlockRow;
use crate::errors::DatabaseError;
use crate::filters::BlockFilter;
use crate::reader::Reader;

/// The value of `totalCountIsEstimate` the reference reports with a listing: the count itself is
/// always exact here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountFlag {
    /// The reference reports an estimate (`true`).
    Estimate,
    /// The reference reports an exact count (`false`).
    Exact,
}

/// A listed block: its row, and the sum of its Core Transfer payments when the listing was asked
/// for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedBlock {
    /// The block's row.
    pub row: BlockRow,
    /// The amounts of the block's Core Transfer payments, summed; `None` unless asked for.
    pub transferred: Option<u128>,
}

impl Reader<'_> {
    /// The page `page` of the blocks that `filter` matches, in the order `order`, with every
    /// match's count and `count` as its estimate flag; `transfers` also sums each block's Core
    /// Transfer payments.
    pub fn blocks(
        &self,
        filter: &BlockFilter,
        order: &[Order],
        page: Page,
        transfers: bool,
        count: CountFlag,
    ) -> Result<Listing<ListedBlock>, DatabaseError> {
        let _ = (filter, order, page, transfers, count);
        todo!("the block listing")
    }
}
