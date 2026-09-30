//! The state tables: one record per wallet, the record of the tip's state, and one undo record
//! per block above the genesis block. The records are text that `heartwood-state` writes and
//! reads; this crate stores them as they are.

use heartwood_crypto::BlockId;
use rusqlite::Connection;

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::reader::Reader;
use crate::values::{Columns, int};

/// A row of `wallets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletRow {
    /// The address.
    pub address: String,
    /// The wallet's record.
    pub record: String,
}

/// The row of `state`: the tip's state besides the wallets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRow {
    /// The height of the tip.
    pub height: u32,
    /// The id of the tip.
    pub block_id: BlockId,
    /// The state's record.
    pub record: String,
}

/// The rows a store holds above a height (blocks, their transactions, missed-slot rows) and above
/// a round (round rows), with its highest round: what a rollback to that height deletes, and,
/// above 0, the store's own counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowCounts {
    /// The blocks above the height.
    pub blocks: u64,
    /// The transactions of those blocks.
    pub transactions: u64,
    /// The missed-slot rows above the height.
    pub missed: u64,
    /// The round rows above the round.
    pub round_rows: u64,
    /// The highest round with rows in the store, 0 when there is none.
    pub last_round: u64,
}

/// A row of `undo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRow {
    /// The height of the block.
    pub height: u32,
    /// The block's undo record.
    pub record: String,
}

impl Database {
    /// Every wallet row, in the order the rows were first inserted.
    pub fn wallets(&self) -> Result<Vec<WalletRow>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT address, record FROM wallets ORDER BY rowid")?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("wallets", row);
            out.push(WalletRow {
                address: columns.get(0)?,
                record: columns.get(1)?,
            });
        }
        Ok(out)
    }

    /// The state row, if the store has one.
    pub fn state_row(&self) -> Result<Option<StateRow>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT height, block_id, record FROM state WHERE id = 1")?;
        let mut rows = statement.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let columns = Columns::new("state", row);
        Ok(Some(StateRow {
            height: columns.uint(0)?,
            block_id: columns.block_id(1)?,
            record: columns.get(2)?,
        }))
    }

    /// Every undo row, by height.
    pub fn undo_records(&self) -> Result<Vec<UndoRow>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT height, record FROM undo ORDER BY height")?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("undo", row);
            out.push(UndoRow {
                height: columns.uint(0)?,
                record: columns.get(1)?,
            });
        }
        Ok(out)
    }
}

impl Chunk<'_> {
    /// Insert or replace the record of the wallet at `address`. A wallet that already has a row
    /// keeps its place in the order of [`Database::wallets`].
    pub fn put_wallet(&mut self, address: &str, record: &str) -> Result<(), DatabaseError> {
        self.transaction
            .prepare_cached(
                "INSERT INTO wallets (address, record) VALUES (?1, ?2) \
                 ON CONFLICT (address) DO UPDATE SET record = excluded.record",
            )?
            .execute([address, record])?;
        Ok(())
    }

    /// Delete the record of the wallet at `address`, if there is one.
    pub fn delete_wallet(&mut self, address: &str) -> Result<(), DatabaseError> {
        self.transaction
            .prepare_cached("DELETE FROM wallets WHERE address = ?1")?
            .execute([address])?;
        Ok(())
    }

    /// Delete every wallet row, before the wallets are written again.
    pub fn delete_wallets(&mut self) -> Result<(), DatabaseError> {
        self.transaction.execute("DELETE FROM wallets", [])?;
        Ok(())
    }

    /// Delete the state row and every wallet row, so that the next start finds no state and
    /// restores by a replay from the genesis block (a rollback's store change).
    pub fn delete_state(&mut self) -> Result<(), DatabaseError> {
        self.transaction
            .execute_batch("DELETE FROM state; DELETE FROM wallets;")?;
        Ok(())
    }

    /// The rows above `height` and `round` in this chunk's view of the store.
    pub fn row_counts(&self, height: u32, round: u64) -> Result<RowCounts, DatabaseError> {
        row_counts(&self.transaction, height, round)
    }

    /// Replace the state row.
    pub fn put_state(&mut self, row: &StateRow) -> Result<(), DatabaseError> {
        self.transaction
            .prepare_cached(
                "INSERT OR REPLACE INTO state (id, height, block_id, record) VALUES (1, ?1, ?2, ?3)",
            )?
            .execute(rusqlite::params![row.height, row.block_id.to_hex(), row.record])?;
        Ok(())
    }

    /// Insert or replace the undo record of the block at `height`.
    pub fn put_undo(&mut self, height: u32, record: &str) -> Result<(), DatabaseError> {
        self.transaction
            .prepare_cached(
                "INSERT INTO undo (height, record) VALUES (?1, ?2) \
                 ON CONFLICT (height) DO UPDATE SET record = excluded.record",
            )?
            .execute(rusqlite::params![height, record])?;
        Ok(())
    }
}

impl Reader<'_> {
    /// The rows above `height` and `round`, read in one read transaction.
    pub fn row_counts(&self, height: u32, round: u64) -> Result<RowCounts, DatabaseError> {
        self.timed(|connection| {
            let read = connection.unchecked_transaction()?;
            row_counts(&read, height, round)
        })
    }
}

/// The rows above `height` and `round` that `connection` reads.
fn row_counts(
    connection: &Connection,
    height: u32,
    round: u64,
) -> Result<RowCounts, DatabaseError> {
    let count = |sql: &str, value: i64| -> Result<u64, DatabaseError> {
        let n: i64 = (connection.prepare_cached(sql)?).query_row([value], |row| row.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    };
    let (height, round) = (i64::from(height), int("rounds.round", round)?);
    Ok(RowCounts {
        blocks: count("SELECT COUNT(*) FROM blocks WHERE height > ?1", height)?,
        transactions: count(
            "SELECT COUNT(*) FROM transactions WHERE block_height > ?1",
            height,
        )?,
        missed: count(
            "SELECT COUNT(*) FROM missed_blocks WHERE height > ?1",
            height,
        )?,
        round_rows: count("SELECT COUNT(*) FROM rounds WHERE round > ?1", round)?,
        last_round: count(
            "SELECT COALESCE(MAX(round), 0) FROM rounds WHERE round > ?1",
            0,
        )?,
    })
}
