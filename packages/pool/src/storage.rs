//! `pool.sqlite`: the admitted transactions, in the reference's table, so that a restarted node
//! adds them again.
//!
//! The table is `pool_20220908 (n INTEGER PRIMARY KEY AUTOINCREMENT, height, id UNIQUE,
//! recipientId, senderId, serialised)` in WAL mode, with `synchronous = NORMAL`. A row is written
//! only once its transaction has been admitted. When the clear marker exists at open, it is
//! removed together with the file and its `-wal` and `-shm` files, and the pool starts empty.

use std::path::Path;

use heartwood_crypto::TransactionId;

use crate::errors::StorageError;

/// The name of the reference's table.
pub const TABLE: &str = "pool_20220908";

/// The file name of the clear marker, in the node's temp directory.
pub const CLEAR_MARKER: &str = "clear-pool.lock";

/// A row of the pool's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTransaction {
    /// The tip's height when the transaction was admitted.
    pub height: u32,
    /// The transaction id.
    pub id: TransactionId,
    /// The recipients' addresses joined by commas; empty for a type without recipients.
    pub recipient_id: String,
    /// The sender's address.
    pub sender_id: String,
    /// The transaction's bytes.
    pub serialised: Vec<u8>,
}

/// The open `pool.sqlite`.
#[derive(Debug)]
pub struct Storage {
    #[expect(dead_code, reason = "the methods of the storage read it")]
    connection: rusqlite::Connection,
}

impl Storage {
    /// Open the file at `path`, creating it, its directory and the table when needed. When
    /// `clear_marker` names a file that exists, that file, the pool's file and its `-wal` and
    /// `-shm` files are removed first.
    pub fn open(path: &Path, clear_marker: Option<&Path>) -> Result<Storage, StorageError> {
        let _ = (path, clear_marker);
        todo!("open pool.sqlite")
    }

    /// Store an admitted transaction.
    pub fn add(&mut self, row: &StoredTransaction) -> Result<(), StorageError> {
        let _ = row;
        todo!("insert a row")
    }

    /// Whether the transaction `id` is stored.
    pub fn has(&self, id: &TransactionId) -> Result<bool, StorageError> {
        let _ = id;
        todo!("look a row up")
    }

    /// Every row, in the order they were stored.
    pub fn all(&self) -> Result<Vec<StoredTransaction>, StorageError> {
        todo!("read every row")
    }

    /// The id and sender of every row whose height is at most `height`, the newest first.
    pub fn old(&self, height: i64) -> Result<Vec<(TransactionId, String)>, StorageError> {
        let _ = height;
        todo!("read the old rows")
    }

    /// Remove the row of transaction `id`, if there is one.
    pub fn remove(&mut self, id: &TransactionId) -> Result<(), StorageError> {
        let _ = id;
        todo!("delete a row")
    }

    /// Remove every row.
    pub fn flush(&mut self) -> Result<(), StorageError> {
        todo!("delete every row")
    }
}
