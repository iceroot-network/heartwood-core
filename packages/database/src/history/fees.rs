//! The fee statistics of `GET /node/fees`.

use crate::errors::DatabaseError;
use crate::reader::Reader;

/// The fees of one (type group, type) as the reference's query aggregates them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeeStatisticsRow {
    /// The type group.
    pub type_group: u32,
    /// The type.
    pub kind: u16,
    /// `AVG(fee)::int8`.
    pub avg: u64,
    /// The lowest fee.
    pub min: u64,
    /// The highest fee.
    pub max: u64,
    /// The fees' sum.
    pub sum: u128,
    /// The burned fees' sum.
    pub burned: u128,
}

impl Reader<'_> {
    /// With a window (its first and last timestamp, since the network's epoch): one row per
    /// (type group, type) present in it, ordered by type group. Without: one row per kept type from
    /// its last 20 transactions, zeros where it has none, with its burned sum over every block.
    pub fn fee_statistics(
        &self,
        window: Option<(i64, i64)>,
    ) -> Result<Vec<FeeStatisticsRow>, DatabaseError> {
        let _ = window;
        todo!("the fee statistics")
    }
}
