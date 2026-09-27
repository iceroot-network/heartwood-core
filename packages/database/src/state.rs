//! The state tables: one record per wallet, the record of the tip's state, and one undo record
//! per block above the genesis block. The records are text that `heartwood-state` writes and
//! reads; this crate stores them as they are.

use heartwood_crypto::BlockId;

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::values::Columns;

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
