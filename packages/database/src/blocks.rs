//! Block rows: saved with their transactions, deleted above a height on a revert, and read back
//! by height, by id, as the last block or as the newest blocks.

use heartwood_crypto::{BlockId, TransactionId};
use rusqlite::{Connection, Row, params};

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::transactions::TransactionRow;
use crate::values::{Columns, int};

/// The columns of `blocks`, in the order [`read_block`] reads them and `save_block` writes them.
macro_rules! columns {
    () => {
        "height, id, version, timestamp, previous_block, number_of_transactions, total_amount, \
         total_fee, burned_fee, reward, donations, payload_length, payload_hash, \
         generator_public_key, username, block_signature"
    };
}

/// A row of `blocks`: the reference's columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRow {
    /// The block id.
    pub id: BlockId,
    /// The height.
    pub height: u32,
    /// The block version.
    pub version: u32,
    /// Seconds since the network epoch.
    pub timestamp: u32,
    /// The previous block's id, as the block carries it.
    pub previous_block: Option<BlockId>,
    /// The number of transactions.
    pub number_of_transactions: u32,
    /// The sum of the transactions' top-level amounts.
    pub total_amount: u64,
    /// The sum of the transactions' fees.
    pub total_fee: u64,
    /// The burned part of the fees, under the milestone at the block's height.
    pub burned_fee: u64,
    /// The block reward.
    pub reward: u64,
    /// The donations taken from the reward: a JSON object of address and amount (a decimal
    /// string), every entry the block carries, in the milestone's order.
    pub donations: String,
    /// The payload length.
    pub payload_length: u32,
    /// The payload hash, lowercase hex.
    pub payload_hash: String,
    /// The generator's public key, lowercase hex.
    pub generator_public_key: String,
    /// The generator's delegate name; none for the genesis block.
    pub username: Option<String>,
    /// The block signature, lowercase hex.
    pub block_signature: String,
}

/// A block as the store holds it: its row, and its transactions' ids and bytes in block order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredBlock {
    /// The block's row.
    pub row: BlockRow,
    /// Each transaction's id and serialised bytes, by sequence.
    pub transactions: Vec<(TransactionId, Vec<u8>)>,
}

impl Database {
    /// The block with the greatest height, if the store holds any.
    pub fn last_block(&self) -> Result<Option<StoredBlock>, DatabaseError> {
        let sql = concat!(
            "SELECT ",
            columns!(),
            " FROM blocks ORDER BY height DESC LIMIT 1"
        );
        Ok(self.select_blocks(sql, [])?.pop())
    }

    /// The blocks from height `from` to `to`, both included, in height order.
    pub fn blocks(&self, from: u32, to: u32) -> Result<Vec<StoredBlock>, DatabaseError> {
        let sql = concat!(
            "SELECT ",
            columns!(),
            " FROM blocks WHERE height BETWEEN ?1 AND ?2 ORDER BY height"
        );
        self.select_blocks(sql, [from, to])
    }

    /// The block with id `id`.
    pub fn block_by_id(&self, id: &BlockId) -> Result<Option<StoredBlock>, DatabaseError> {
        let sql = concat!("SELECT ", columns!(), " FROM blocks WHERE id = ?1");
        Ok(self.select_blocks(sql, [id.to_hex()])?.pop())
    }

    /// The `count` blocks with the greatest heights, the newest first, as the reference's
    /// `getTopBlocks` gives them.
    pub fn top_blocks(&self, count: u32) -> Result<Vec<StoredBlock>, DatabaseError> {
        let sql = concat!(
            "SELECT ",
            columns!(),
            " FROM blocks ORDER BY height DESC LIMIT ?1"
        );
        self.select_blocks(sql, [count])
    }

    /// The ids of `ids` that a stored block holds, in the order of `ids`.
    pub fn forged_transaction_ids(
        &self,
        ids: &[TransactionId],
    ) -> Result<Vec<TransactionId>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT 1 FROM transactions WHERE id = ?1")?;
        let mut forged = Vec::new();
        for id in ids {
            if statement.exists([id.to_hex()])? {
                forged.push(*id);
            }
        }
        Ok(forged)
    }

    /// The blocks that `sql`, a query of the block columns, selects with `params`, each with its
    /// transactions.
    fn select_blocks(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<StoredBlock>, DatabaseError> {
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut rows = statement.query(params)?;
        let mut blocks = Vec::new();
        while let Some(row) = rows.next()? {
            let row = read_block(row)?;
            let transactions = block_transactions(&self.connection, &row.id)?;
            blocks.push(StoredBlock { row, transactions });
        }
        Ok(blocks)
    }
}

/// A row of `blocks`, selected with its columns in their order.
fn read_block(row: &Row<'_>) -> Result<BlockRow, DatabaseError> {
    let columns = Columns::new("blocks", row);
    Ok(BlockRow {
        height: columns.uint(0)?,
        id: columns.block_id(1)?,
        version: columns.uint(2)?,
        timestamp: columns.uint(3)?,
        previous_block: columns.block_id_or_null(4)?,
        number_of_transactions: columns.uint(5)?,
        total_amount: columns.uint(6)?,
        total_fee: columns.uint(7)?,
        burned_fee: columns.uint(8)?,
        reward: columns.uint(9)?,
        donations: columns.get(10)?,
        payload_length: columns.uint(11)?,
        payload_hash: columns.get(12)?,
        generator_public_key: columns.get(13)?,
        username: columns.get(14)?,
        block_signature: columns.get(15)?,
    })
}

/// The ids and bytes of the transactions of block `id`, by sequence.
fn block_transactions(
    connection: &Connection,
    id: &BlockId,
) -> Result<Vec<(TransactionId, Vec<u8>)>, DatabaseError> {
    let mut statement = connection.prepare_cached(
        "SELECT id, serialised FROM transactions WHERE block_id = ?1 ORDER BY sequence",
    )?;
    let mut rows = statement.query([id.to_hex()])?;
    let mut transactions = Vec::new();
    while let Some(row) = rows.next()? {
        let columns = Columns::new("transactions", row);
        transactions.push((columns.transaction_id(0)?, columns.get(1)?));
    }
    Ok(transactions)
}

impl Chunk<'_> {
    /// Save a block's row and its transactions' rows. After an error some of them may be
    /// written: the caller rolls back the block's savepoint, or drops the chunk.
    pub fn save_block(
        &mut self,
        block: &BlockRow,
        transactions: &[TransactionRow],
    ) -> Result<(), DatabaseError> {
        let connection: &Connection = &self.transaction;
        let sql = concat!(
            "INSERT INTO blocks (",
            columns!(),
            ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        );
        connection.prepare_cached(sql)?.execute(params![
            block.height,
            block.id.to_hex(),
            block.version,
            block.timestamp,
            block.previous_block.as_ref().map(BlockId::to_hex),
            block.number_of_transactions,
            int("blocks.total_amount", block.total_amount)?,
            int("blocks.total_fee", block.total_fee)?,
            int("blocks.burned_fee", block.burned_fee)?,
            int("blocks.reward", block.reward)?,
            block.donations,
            block.payload_length,
            block.payload_hash,
            block.generator_public_key,
            block.username,
            block.block_signature,
        ])?;
        for transaction in transactions {
            crate::transactions::insert(connection, transaction)?;
        }
        Ok(())
    }

    /// Delete what a revert to `height` takes back: the transactions of the blocks above
    /// `height` with their recipients, those blocks, their undo records, the missed rows above `height`, the milestone
    /// entries above `height`, and the round rows above `round`, the round of the block at
    /// `height`.
    pub fn delete_above(&mut self, height: u32, round: u64) -> Result<(), DatabaseError> {
        let connection: &Connection = &self.transaction;
        for sql in [
            "DELETE FROM undo WHERE height > ?1",
            "DELETE FROM milestone_entries WHERE height > ?1",
            "DELETE FROM missed_blocks WHERE height > ?1",
            "DELETE FROM transaction_recipients WHERE transaction_id IN \
             (SELECT id FROM transactions WHERE block_height > ?1)",
            "DELETE FROM transactions WHERE block_id IN (SELECT id FROM blocks WHERE height > ?1)",
            "DELETE FROM blocks WHERE height > ?1",
        ] {
            connection.prepare_cached(sql)?.execute([height])?;
        }
        connection
            .prepare_cached("DELETE FROM rounds WHERE round > ?1")?
            .execute([int("rounds.round", round)?])?;
        Ok(())
    }
}
