//! The burned totals of `GET /blockchain`, over a range of heights, so that a node can grow them as
//! its tip grows.

use crate::errors::DatabaseError;
use crate::reader::Reader;

/// What the blocks of a range of heights burned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BurnedTotals {
    /// The fees the blocks burned.
    pub fees: u128,
    /// The amounts of the blocks' burn transactions.
    pub transactions: u128,
}

impl Reader<'_> {
    /// What the blocks at the heights above `above`, up to and with `to`, burned.
    pub fn burned_totals(&self, above: u32, to: u32) -> Result<BurnedTotals, DatabaseError> {
        let _ = (above, to);
        todo!("the burned totals")
    }
}
