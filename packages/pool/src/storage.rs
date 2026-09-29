//! `pool.sqlite`: the admitted transactions, in the reference's table, so that a restarted node
//! adds them again.
//!
//! The table is `pool_20220908 (n INTEGER PRIMARY KEY AUTOINCREMENT, height, id UNIQUE,
//! recipientId, senderId, serialised)` in WAL mode, with `synchronous = NORMAL`. A row is written
//! only once its transaction has been admitted. When the clear marker exists at open, it is
//! removed together with the file and its `-shm` and `-wal` files, and the pool starts empty.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use heartwood_crypto::TransactionId;
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags, params};

use crate::errors::StorageError;

/// The name of the reference's table.
pub const TABLE: &str = "pool_20220908";

/// The file name of the clear marker, in the node's temp directory.
pub const CLEAR_MARKER: &str = "clear-pool.lock";

/// The reference's schema: the table and its two indexes.
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS pool_20220908 (
        n                  INTEGER      PRIMARY KEY AUTOINCREMENT,
        height             INTEGER      NOT NULL,
        id                 VARCHAR(64)  NOT NULL,
        recipientId        TEXT         NOT NULL,
        senderId           VARCHAR(34)  NOT NULL,
        serialised         BLOB         NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS pool_id ON pool_20220908 (id);
    CREATE INDEX IF NOT EXISTS pool_height ON pool_20220908 (height);
";

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
    connection: Connection,
}

impl Storage {
    /// Open the file at `path`, creating it, its directory and the table when needed. When
    /// `clear_marker` names a file that exists, that file, the pool's file and its `-shm` and
    /// `-wal` files are removed first, in that order; as in the reference, the first file that
    /// cannot be removed (because it does not exist) ends the removal.
    pub fn open(path: &Path, clear_marker: Option<&Path>) -> Result<Storage, StorageError> {
        if let Some(marker) = clear_marker.filter(|marker| marker.exists()) {
            // The reference ignores a marker it cannot remove, and stops at the first pool file.
            let _ = fs::remove_file(marker);
            for file in [
                path.to_path_buf(),
                suffixed(path, "-shm"),
                suffixed(path, "-wal"),
            ] {
                if fs::remove_file(&file).is_err() {
                    break;
                }
            }
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|error| StorageError::File {
                path: parent.to_path_buf(),
                detail: error.to_string(),
            })?;
        }
        // As the store opens its file: no URI file names (the path is a path), and a schema that
        // is only this file's.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(path, flags)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        // `journal_mode` answers with the mode it set, so it is read as a query.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        connection.execute_batch("PRAGMA synchronous = NORMAL;")?;
        connection.execute_batch(SCHEMA)?;
        Ok(Storage { connection })
    }

    /// Store an admitted transaction.
    pub fn add(&mut self, row: &StoredTransaction) -> Result<(), StorageError> {
        self.connection
            .prepare_cached(
                "INSERT INTO pool_20220908 (height, id, recipientId, senderId, serialised) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![
                row.height,
                row.id.to_hex(),
                row.recipient_id,
                row.sender_id,
                row.serialised
            ])?;
        Ok(())
    }

    /// Whether the transaction `id` is stored.
    pub fn has(&self, id: &TransactionId) -> Result<bool, StorageError> {
        let count: i64 = self
            .connection
            .prepare_cached("SELECT COUNT(*) FROM pool_20220908 WHERE id = ?1")?
            .query_row([id.to_hex()], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Every row, in the order they were stored.
    pub fn all(&self) -> Result<Vec<StoredTransaction>, StorageError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT height, id, recipientId, senderId, serialised FROM pool_20220908 ORDER BY n",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        let mut stored = Vec::new();
        for row in rows {
            let (height, id, recipient_id, sender_id, serialised) = row?;
            stored.push(StoredTransaction {
                height: u32::try_from(height)
                    .map_err(|_| StorageError::CorruptRow(format!("height {height} of {id}")))?,
                id: parse_id(&id)?,
                recipient_id,
                sender_id,
                serialised,
            });
        }
        Ok(stored)
    }

    /// The id and sender of every row whose height is at most `height`, the newest first.
    pub fn old(&self, height: i64) -> Result<Vec<(TransactionId, String)>, StorageError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, senderId FROM pool_20220908 WHERE height <= ?1 ORDER BY n DESC",
        )?;
        let rows = statement.query_map([height], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut old = Vec::new();
        for row in rows {
            let (id, sender) = row?;
            old.push((parse_id(&id)?, sender));
        }
        Ok(old)
    }

    /// Remove the row of transaction `id`, if there is one.
    pub fn remove(&mut self, id: &TransactionId) -> Result<(), StorageError> {
        self.connection
            .prepare_cached("DELETE FROM pool_20220908 WHERE id = ?1")?
            .execute([id.to_hex()])?;
        Ok(())
    }

    /// Remove every row.
    pub fn flush(&mut self) -> Result<(), StorageError> {
        self.connection.execute("DELETE FROM pool_20220908", [])?;
        Ok(())
    }
}

/// `path` with `suffix` added to its file name.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

/// A stored id, which the pool wrote as 64 hex digits.
fn parse_id(id: &str) -> Result<TransactionId, StorageError> {
    TransactionId::from_hex(id).map_err(|_| StorageError::CorruptRow(format!("id {id:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u8, height: u32) -> StoredTransaction {
        StoredTransaction {
            height,
            id: TransactionId::from_array([n; 32]),
            recipient_id: if n.is_multiple_of(2) {
                String::new()
            } else {
                format!("r{n},s{n}")
            },
            sender_id: format!("sender{n}"),
            serialised: vec![n; usize::from(n) + 1],
        }
    }

    #[test]
    fn the_file_is_opened_as_the_store_opens_its_own() {
        let dir = tempdir("flags");
        let storage = Storage::open(&dir.join("pool.sqlite"), None).unwrap();
        let config = |flag| storage.connection.db_config(flag).unwrap();
        assert!(config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE));
        assert!(!config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA));
    }

    #[test]
    fn rows_round_trip_in_storage_order() {
        let dir = tempdir("round-trip");
        let path = dir.join("pool").join("pool.sqlite");
        let mut storage = Storage::open(&path, None).unwrap();
        for (n, height) in [(3, 10), (1, 12), (2, 11)] {
            storage.add(&row(n, height)).unwrap();
        }
        assert_eq!(
            storage.all().unwrap(),
            vec![row(3, 10), row(1, 12), row(2, 11)]
        );
        assert!(storage.has(&row(1, 0).id).unwrap());
        assert!(!storage.has(&row(4, 0).id).unwrap());
        // Rows at or below a height, the newest first.
        let old = storage.old(11).unwrap();
        assert_eq!(
            old,
            vec![
                (row(2, 0).id, "sender2".into()),
                (row(3, 0).id, "sender3".into())
            ]
        );
        // The id is unique.
        assert!(matches!(
            storage.add(&row(3, 99)),
            Err(StorageError::Sqlite(_))
        ));
        storage.remove(&row(3, 0).id).unwrap();
        storage.remove(&row(9, 0).id).unwrap();
        drop(storage);
        let mut storage = Storage::open(&path, None).unwrap();
        assert_eq!(storage.all().unwrap(), vec![row(1, 12), row(2, 11)]);
        storage.flush().unwrap();
        assert!(storage.all().unwrap().is_empty());
        let mode: String = storage
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn the_clear_marker_empties_the_storage() {
        let dir = tempdir("clear");
        let path = dir.join("pool.sqlite");
        let marker = dir.join(CLEAR_MARKER);
        let mut storage = Storage::open(&path, Some(&marker)).unwrap();
        storage.add(&row(1, 1)).unwrap();
        drop(storage);
        // No marker: the rows stay.
        assert_eq!(
            Storage::open(&path, Some(&marker))
                .unwrap()
                .all()
                .unwrap()
                .len(),
            1
        );
        fs::write(&marker, b"").unwrap();
        let storage = Storage::open(&path, Some(&marker)).unwrap();
        assert!(storage.all().unwrap().is_empty());
        assert!(!marker.exists());
        // A marker without a pool file: nothing to remove, an empty pool.
        drop(storage);
        fs::remove_file(&path).unwrap();
        fs::write(&marker, b"").unwrap();
        assert!(
            Storage::open(&path, Some(&marker))
                .unwrap()
                .all()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn corrupt_rows_are_typed_errors() {
        let dir = tempdir("corrupt");
        let storage = Storage::open(&dir.join("pool.sqlite"), None).unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO pool_20220908 (height, id, recipientId, senderId, serialised) \
                 VALUES (1, 'not hex', '', 's', x'00')",
                [],
            )
            .unwrap();
        assert!(matches!(storage.all(), Err(StorageError::CorruptRow(_))));
        assert!(matches!(storage.old(5), Err(StorageError::CorruptRow(_))));
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "heartwood-pool-storage-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
