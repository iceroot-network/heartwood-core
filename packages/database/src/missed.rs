//! Missed-slot rows: the slots in which the scheduled delegate forged no block.

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::values::Columns;

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
        let mut statement = self.connection.prepare_cached(
            "SELECT timestamp, height, username FROM missed_blocks ORDER BY timestamp",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("missed_blocks", row);
            out.push(MissedRow {
                timestamp: columns.get(0)?,
                height: columns.uint(1)?,
                username: columns.get(2)?,
            });
        }
        Ok(out)
    }

    /// Whether the table holds a row, as the reference's `hasMissedBlocks` asks at a start.
    pub fn has_missed_blocks(&self) -> Result<bool, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM missed_blocks)")?;
        Ok(statement.query_row([], |row| row.get(0))?)
    }
}

impl Chunk<'_> {
    /// Save the missed rows of an applied block. A row of the same slot is replaced, as the
    /// reference's `save` replaces an entity with the same primary key.
    pub fn save_missed(&mut self, rows: &[MissedRow]) -> Result<(), DatabaseError> {
        let mut statement = self.transaction.prepare_cached(
            "INSERT INTO missed_blocks (timestamp, height, username) VALUES (?1, ?2, ?3) \
             ON CONFLICT (timestamp) DO UPDATE SET height = excluded.height, \
             username = excluded.username",
        )?;
        for row in rows {
            statement.execute(rusqlite::params![row.timestamp, row.height, row.username])?;
        }
        Ok(())
    }
}
