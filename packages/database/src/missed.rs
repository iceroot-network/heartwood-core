//! Missed-slot rows: the slots in which the scheduled delegate forged no block.

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;

/// A row of `missed_blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedRow {
    /// The first second of the missed slot, since the network epoch.
    pub timestamp: i64,
    /// The height of the block forged after the missed slots.
    pub height: u32,
    /// The delegate that missed the slot.
    pub username: String,
}

impl Database {
    /// Every missed row, by timestamp.
    pub fn missed_blocks(&self) -> Result<Vec<MissedRow>, DatabaseError> {
        todo!("read the missed rows")
    }
}

impl Chunk<'_> {
    /// Save the missed rows of an applied block.
    pub fn save_missed(&mut self, rows: &[MissedRow]) -> Result<(), DatabaseError> {
        let _ = rows;
        todo!("insert the missed rows")
    }
}
