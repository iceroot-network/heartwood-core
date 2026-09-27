//! Transaction rows, written with their block (see [`crate::Chunk::save_block`]) and read back
//! with it (see [`crate::StoredBlock`]) or on their own ([`Database::transaction_rows`]).

use heartwood_crypto::{BlockId, TransactionId};
use rusqlite::{Connection, Row, params};

use crate::connection::Database;
use crate::errors::DatabaseError;
use crate::values::{Columns, int};

/// The columns of `transactions`, in the order [`read`] reads them and [`insert`] writes them.
macro_rules! columns {
    () => {
        "id, version, block_id, block_height, sequence, timestamp, nonce, sender_id, \
         sender_public_key, recipient_id, type, type_group, memo, amount, fee, burned_fee, \
         serialised, asset"
    };
}

/// A row of `transactions`: the reference's columns, as its model converter writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRow {
    /// The transaction id.
    pub id: TransactionId,
    /// The version.
    pub version: u8,
    /// The id of the block that holds it.
    pub block_id: BlockId,
    /// The height of that block.
    pub block_height: u32,
    /// The position in the block, from 0.
    pub sequence: u32,
    /// The block's timestamp.
    pub timestamp: u32,
    /// The sender's nonce.
    pub nonce: u64,
    /// The sender's address.
    pub sender_id: String,
    /// The sender's public key, lowercase hex.
    pub sender_public_key: String,
    /// The recipient, for the types where the reference sets one.
    pub recipient_id: Option<String>,
    /// The type within its type group.
    pub kind: u16,
    /// The type group.
    pub type_group: u32,
    /// The memo's bytes, if any.
    pub memo: Option<Vec<u8>>,
    /// The top-level amount (0 for a transfer, whose items are in `asset`).
    pub amount: u64,
    /// The fee.
    pub fee: u64,
    /// The burned part of the fee.
    pub burned_fee: u64,
    /// The bytes the block carries.
    pub serialised: Vec<u8>,
    /// The asset as the reference's JSON, if the transaction has one.
    pub asset: Option<String>,
}

impl Database {
    /// Every transaction row of the block `block`, by sequence.
    pub fn transaction_rows(&self, block: &BlockId) -> Result<Vec<TransactionRow>, DatabaseError> {
        let sql = concat!(
            "SELECT ",
            columns!(),
            " FROM transactions WHERE block_id = ?1 ORDER BY sequence"
        );
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut rows = statement.query([block.to_hex()])?;
        let mut transactions = Vec::new();
        while let Some(row) = rows.next()? {
            transactions.push(read(row)?);
        }
        Ok(transactions)
    }
}

/// A row of `transactions`, selected with its columns in their order.
fn read(row: &Row<'_>) -> Result<TransactionRow, DatabaseError> {
    let columns = Columns::new("transactions", row);
    Ok(TransactionRow {
        id: columns.transaction_id(0)?,
        version: columns.uint(1)?,
        block_id: columns.block_id(2)?,
        block_height: columns.uint(3)?,
        sequence: columns.uint(4)?,
        timestamp: columns.uint(5)?,
        nonce: columns.uint(6)?,
        sender_id: columns.get(7)?,
        sender_public_key: columns.get(8)?,
        recipient_id: columns.get(9)?,
        kind: columns.uint(10)?,
        type_group: columns.uint(11)?,
        memo: columns.get(12)?,
        amount: columns.uint(13)?,
        fee: columns.uint(14)?,
        burned_fee: columns.uint(15)?,
        serialised: columns.get(16)?,
        asset: columns.get(17)?,
    })
}

/// Insert `row` into `transactions`.
pub(crate) fn insert(connection: &Connection, row: &TransactionRow) -> Result<(), DatabaseError> {
    let sql = concat!(
        "INSERT INTO transactions (",
        columns!(),
        ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
    );
    connection.prepare_cached(sql)?.execute(params![
        row.id.to_hex(),
        row.version,
        row.block_id.to_hex(),
        row.block_height,
        row.sequence,
        row.timestamp,
        int("transactions.nonce", row.nonce)?,
        row.sender_id,
        row.sender_public_key,
        row.recipient_id,
        row.kind,
        row.type_group,
        row.memo,
        int("transactions.amount", row.amount)?,
        int("transactions.fee", row.fee)?,
        int("transactions.burned_fee", row.burned_fee)?,
        row.serialised,
        row.asset,
    ])?;
    Ok(())
}
