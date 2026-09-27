//! The state tables: one record per wallet, the record of the tip's state, and one undo record
//! per block above the genesis block. The records are text that `heartwood-state` writes and
//! reads; this crate stores them as they are.

use heartwood_crypto::BlockId;

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;

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
        todo!("read the wallet rows")
    }

    /// The state row, if the store has one.
    pub fn state_row(&self) -> Result<Option<StateRow>, DatabaseError> {
        todo!("read the state row")
    }

    /// Every undo row, by height.
    pub fn undo_records(&self) -> Result<Vec<UndoRow>, DatabaseError> {
        todo!("read the undo rows")
    }
}

impl Chunk<'_> {
    /// Insert or replace the record of the wallet at `address`.
    pub fn put_wallet(&mut self, address: &str, record: &str) -> Result<(), DatabaseError> {
        let _ = (address, record);
        todo!("upsert a wallet row")
    }

    /// Delete the record of the wallet at `address`, if there is one.
    pub fn delete_wallet(&mut self, address: &str) -> Result<(), DatabaseError> {
        let _ = address;
        todo!("delete a wallet row")
    }

    /// Delete every wallet row, before the wallets are written again.
    pub fn delete_wallets(&mut self) -> Result<(), DatabaseError> {
        todo!("delete the wallet rows")
    }

    /// Replace the state row.
    pub fn put_state(&mut self, row: &StateRow) -> Result<(), DatabaseError> {
        let _ = row;
        todo!("write the state row")
    }

    /// Insert or replace the undo record of the block at `height`.
    pub fn put_undo(&mut self, height: u32, record: &str) -> Result<(), DatabaseError> {
        let _ = (height, record);
        todo!("write an undo row")
    }
}
