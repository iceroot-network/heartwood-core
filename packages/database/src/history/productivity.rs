//! The reference's `getBlockProductivity`: each delegate's produced and missed blocks since the
//! start of the window, as its full outer join gives them, before any rounding.

use crate::errors::DatabaseError;
use crate::reader::Reader;

/// One delegate's counts in the window: the produced blocks and the missed slots, each `None` where
/// the join has no row on its side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductivityRow {
    /// The delegate's name.
    pub username: String,
    /// The blocks it produced in the window.
    pub produced: Option<u64>,
    /// The slots it missed in the window.
    pub missed: Option<u64>,
}

impl Reader<'_> {
    /// Every delegate's produced and missed counts since `since_epoch_seconds` (seconds since the
    /// network's epoch).
    pub fn productivity(
        &self,
        since_epoch_seconds: i64,
    ) -> Result<Vec<ProductivityRow>, DatabaseError> {
        let _ = since_epoch_seconds;
        todo!("the productivity query")
    }
}
