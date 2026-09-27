//! Block rows: saved with their transactions, deleted above a height on a revert, and read back
//! by height, by id, as the last block or as the newest blocks.

use heartwood_crypto::{BlockId, TransactionId};

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::transactions::TransactionRow;

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
        todo!("read the last block")
    }

    /// The blocks from height `from` to `to`, both included, in height order.
    pub fn blocks(&self, from: u32, to: u32) -> Result<Vec<StoredBlock>, DatabaseError> {
        let _ = (from, to);
        todo!("read a range of blocks")
    }

    /// The block with id `id`.
    pub fn block_by_id(&self, id: &BlockId) -> Result<Option<StoredBlock>, DatabaseError> {
        let _ = id;
        todo!("read a block by id")
    }

    /// The `count` blocks with the greatest heights, the newest first, as the reference's
    /// `getTopBlocks` gives them.
    pub fn top_blocks(&self, count: u32) -> Result<Vec<StoredBlock>, DatabaseError> {
        let _ = count;
        todo!("read the newest blocks")
    }

    /// The ids of `ids` that a stored block holds, in the order of `ids`.
    pub fn forged_transaction_ids(
        &self,
        ids: &[TransactionId],
    ) -> Result<Vec<TransactionId>, DatabaseError> {
        let _ = ids;
        todo!("look the ids up")
    }
}

impl Chunk<'_> {
    /// Save a block's row and its transactions' rows.
    pub fn save_block(
        &mut self,
        block: &BlockRow,
        transactions: &[TransactionRow],
    ) -> Result<(), DatabaseError> {
        let _ = (block, transactions);
        todo!("insert the rows")
    }

    /// Delete what a revert to `height` takes back: the transactions of the blocks above
    /// `height`, those blocks, their undo records, the missed rows above `height`, the milestone
    /// entries above `height`, and the round rows above `round`, the round of the block at
    /// `height`.
    pub fn delete_above(&mut self, height: u32, round: u64) -> Result<(), DatabaseError> {
        let _ = (height, round);
        todo!("delete the rows above the new tip")
    }
}
