//! The writer connection, the schema, and chunks.
//!
//! A node has one [`Database`], which owns the one writer connection to its file, opened in WAL
//! mode with `synchronous = FULL` (a committed chunk survives a power loss), `foreign_keys = ON`,
//! `busy_timeout = 5000`, `mmap_size = 0`, an 8 MB page cache and a 16 MB limit on the WAL file
//! kept after a checkpoint; the writer checkpoints the log into the file when it closes, even
//! while read-only connections stay open. SQLite's defensive mode is on and the schema is not
//! trusted, so neither a changed schema nor an SQL function can reach outside the tables;
//! extensions are never loaded. An empty file gets schema version 3 ([`SCHEMA`], [`SCHEMA_2`], then
//! [`SCHEMA_3`]); a file of version 1 is upgraded in place, in one transaction, by adding the
//! recipients side table and filling it from the stored transactions ([`crate::recipients`]), and
//! then the indices of version 3; a file of version 2 gains those indices in place; a file of
//! another version, or a file that is not a node's database, is refused.
//!
//! Everything a chunk of blocks changes is written in one SQLite transaction, a [`Chunk`]: its
//! rows are all stored, or, after a crash or when the chunk is dropped, none. Each block of the
//! chunk has its own [`Savepoint`], so that a block refused halfway leaves the chunk's other
//! blocks.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior};

use crate::errors::DatabaseError;

/// The schema version this build creates and reads.
pub const SCHEMA_VERSION: u32 = 3;

/// Schema version 1: every table and index, as SQL statements.
pub const SCHEMA: &str = include_str!("schema.sql");

/// The step from schema version 1 to 2: the recipients side table and its index.
pub const SCHEMA_2: &str = include_str!("schema_2.sql");

/// The step from schema version 2 to 3: the indices of the listing routes' queries (indices only).
pub const SCHEMA_3: &str = include_str!("schema_3.sql");

/// The prepared statements the connection keeps.
const STATEMENTS: usize = 64;

/// Where a node's database is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The database file.
    pub path: PathBuf,
}

impl Settings {
    /// The database of a node whose data directory is `data`: `database/blockchain.db` in it.
    pub fn in_data_dir(data: &Path) -> Settings {
        Settings {
            path: data.join("database").join("blockchain.db"),
        }
    }
}

/// A node's database, with its one writer connection.
///
/// The reads here run on the writer connection, so they see every committed chunk. The readers
/// of later components (the API) open their own read-only connections.
#[derive(Debug)]
pub struct Database {
    pub(crate) connection: Connection,
}

impl Database {
    /// Open the database of `settings`, creating the file, its directory and the schema when the
    /// file does not exist or is empty, and upgrading a file of schema version 1. A file of
    /// another schema version is refused ([`DatabaseError::SchemaVersion`]).
    pub fn open(settings: &Settings) -> Result<Database, DatabaseError> {
        if let Some(dir) = settings
            .path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
        {
            std::fs::create_dir_all(dir).map_err(|error| DatabaseError::Directory {
                path: dir.to_path_buf(),
                detail: error.to_string(),
            })?;
        }
        // No URI file names: the path is a path.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&settings.path, flags).map_err(open_error)?;
        let mode = configure(&connection).map_err(open_error)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(DatabaseError::Sqlite(format!(
                "the file cannot use WAL mode (journal mode {mode})"
            )));
        }
        // Every statement of the store stays prepared: they are fewer than this.
        connection.set_prepared_statement_cache_capacity(STATEMENTS);
        let mut database = Database { connection };
        database.check_schema()?;
        Ok(database)
    }

    /// Create the schema in an empty file, upgrade a file of version 1 or 2, or check the version
    /// of a file that has the current one.
    fn check_schema(&mut self) -> Result<(), DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let objects: i64 =
            transaction.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |row| row.get(0))?;
        match (version, objects) {
            (0, 0) => {
                transaction.execute_batch(SCHEMA)?;
                transaction.execute_batch(SCHEMA_2)?;
                transaction.execute_batch(SCHEMA_3)?;
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            (1 | 2, _) => {
                if version == 1 {
                    transaction.execute_batch(SCHEMA_2)?;
                    crate::recipients::upgrade(&transaction)?;
                }
                transaction.execute_batch(SCHEMA_3)?;
                transaction.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
                    [SCHEMA_VERSION.to_string()],
                )?;
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            (0, _) => {
                return Err(DatabaseError::SchemaVersion {
                    found: "none (the file holds tables of another program)".to_owned(),
                    expected: SCHEMA_VERSION,
                });
            }
            (version, _) if version == i64::from(SCHEMA_VERSION) => {
                let stored: Option<String> = transaction
                    .query_row(
                        "SELECT value FROM meta WHERE key = 'schema_version'",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(stored) = stored.filter(|stored| *stored != SCHEMA_VERSION.to_string())
                {
                    return Err(DatabaseError::SchemaVersion {
                        found: stored,
                        expected: SCHEMA_VERSION,
                    });
                }
            }
            (version, _) => {
                return Err(DatabaseError::SchemaVersion {
                    found: version.to_string(),
                    expected: SCHEMA_VERSION,
                });
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Start a chunk: one SQLite transaction for everything the chunk's blocks change. Dropping
    /// the chunk without [`Chunk::commit`] rolls every change back.
    pub fn begin(&mut self) -> Result<Chunk<'_>, DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Ok(Chunk { transaction })
    }

    /// Delete every row of every table, in one transaction: the store of a node that starts again
    /// from its genesis block, whose first start writes every table again, `meta` included.
    pub fn reset(&mut self) -> Result<(), DatabaseError> {
        let chunk = self.begin()?;
        chunk.transaction.execute_batch(
            "DELETE FROM undo; DELETE FROM transaction_recipients; DELETE FROM transactions;
             DELETE FROM blocks; DELETE FROM rounds; DELETE FROM missed_blocks; DELETE FROM wallets;
             DELETE FROM state;
             DELETE FROM milestone_entries; DELETE FROM meta; DELETE FROM delegate_versions;",
        )?;
        chunk.commit()
    }
}

impl Drop for Database {
    /// Copy the log's committed pages into the database file as the writer closes: SQLite does
    /// so only when the last connection closes, and read-only connections may outlive the writer.
    /// A passive checkpoint never waits for a reader; what it leaves, the next open replays.
    fn drop(&mut self) {
        let _ = (self.connection).query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    }
}

/// Set the connection's options; the journal mode it then has.
fn configure(connection: &Connection) -> rusqlite::Result<String> {
    connection.busy_timeout(Duration::from_millis(5000))?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    let mode = connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "mmap_size", 0)?;
    connection.pragma_update(None, "cache_size", -8000)?;
    connection.pragma_update(None, "journal_size_limit", 16 * 1024 * 1024)?;
    Ok(mode)
}

/// An error of opening the file: a file that is not an SQLite database is not a node's database
/// either.
fn open_error(error: rusqlite::Error) -> DatabaseError {
    if error.sqlite_error_code() == Some(ErrorCode::NotADatabase) {
        DatabaseError::SchemaVersion {
            found: "none (the file is not an SQLite database)".to_owned(),
            expected: SCHEMA_VERSION,
        }
    } else {
        DatabaseError::from(error)
    }
}

/// One chunk of blocks: a transaction on the writer connection. Its rows become visible, all at
/// once, when it is committed; dropping it rolls them back.
#[derive(Debug)]
pub struct Chunk<'d> {
    pub(crate) transaction: rusqlite::Transaction<'d>,
}

impl<'d> Chunk<'d> {
    /// Start one block's savepoint. The block's writes go through it; committing it keeps them in
    /// the chunk, and dropping it (a refused block) rolls back to the state of the chunk before
    /// the block.
    pub fn savepoint(&mut self) -> Result<Savepoint<'_, 'd>, DatabaseError> {
        self.transaction.execute_batch("SAVEPOINT block")?;
        Ok(Savepoint {
            chunk: self,
            open: true,
        })
    }

    /// Commit the chunk: every row it wrote is stored, durably, before this returns.
    pub fn commit(self) -> Result<(), DatabaseError> {
        self.transaction.commit()?;
        Ok(())
    }
}

/// One block's savepoint inside a [`Chunk`]. It gives access to the chunk's writes; dropping it
/// without [`Savepoint::commit`] rolls back what was written since it started.
///
/// Savepoints nest (a savepoint derefs to its chunk), and the borrows make them end in the
/// reverse order of their starts, which is the order SQLite requires.
#[derive(Debug)]
pub struct Savepoint<'c, 'd> {
    chunk: &'c mut Chunk<'d>,
    open: bool,
}

impl Savepoint<'_, '_> {
    /// Keep the block's writes in the chunk.
    pub fn commit(mut self) -> Result<(), DatabaseError> {
        self.open = false;
        self.chunk.transaction.execute_batch("RELEASE block")?;
        Ok(())
    }

    /// Roll back what was written since the savepoint started.
    pub fn rollback(mut self) -> Result<(), DatabaseError> {
        self.open = false;
        self.chunk
            .transaction
            .execute_batch("ROLLBACK TO block; RELEASE block")?;
        Ok(())
    }
}

impl Drop for Savepoint<'_, '_> {
    fn drop(&mut self) {
        if self.open {
            // Nothing can report an error from here. If the rollback fails, SQLite has already
            // rolled back the whole chunk, whose commit then fails.
            let _ = self
                .chunk
                .transaction
                .execute_batch("ROLLBACK TO block; RELEASE block");
        }
    }
}

impl<'d> Deref for Savepoint<'_, 'd> {
    type Target = Chunk<'d>;

    fn deref(&self) -> &Chunk<'d> {
        self.chunk
    }
}

impl<'d> DerefMut for Savepoint<'_, 'd> {
    fn deref_mut(&mut self) -> &mut Chunk<'d> {
        self.chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A method of the same name as rusqlite's `Connection::load_extension_enable`, which exists
    /// only with rusqlite's `load_extension` feature. While the feature is off, a call resolves
    /// to this method; with the feature on it resolves to rusqlite's inherent (and `unsafe`)
    /// method, and the test below no longer compiles.
    trait NoExtensions {
        fn load_extension_enable(&self) -> &'static str;
    }

    impl NoExtensions for Connection {
        fn load_extension_enable(&self) -> &'static str {
            "rusqlite is built without extension loading"
        }
    }

    /// A new database in the system's temporary directory; removed when the test ends.
    struct Scratch {
        database: Option<Database>,
        dir: PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            drop(self.database.take());
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "heartwood-database-unit-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let database = Database::open(&Settings::in_data_dir(&dir)).unwrap();
        Scratch {
            database: Some(database),
            dir,
        }
    }

    #[test]
    fn the_writer_connection_has_the_documented_options() {
        let scratch = scratch("options");
        let database = scratch.database.as_ref().unwrap();
        let pragma = |name: &str| -> String {
            database
                .connection
                .pragma_query_value(None, name, |row| row.get::<_, rusqlite::types::Value>(0))
                .map(|value| format!("{value:?}"))
                .unwrap()
        };
        assert_eq!(pragma("journal_mode"), r#"Text("wal")"#);
        assert_eq!(pragma("synchronous"), "Integer(2)");
        assert_eq!(pragma("foreign_keys"), "Integer(1)");
        assert_eq!(pragma("busy_timeout"), "Integer(5000)");
        assert_eq!(pragma("mmap_size"), "Integer(0)");
        assert_eq!(pragma("cache_size"), "Integer(-8000)");
        assert_eq!(pragma("journal_size_limit"), "Integer(16777216)");
        assert_eq!(pragma("trusted_schema"), "Integer(0)");
        assert!(
            database
                .connection
                .db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE)
                .unwrap()
        );
    }

    #[test]
    fn extensions_cannot_be_loaded() {
        // At build time: rusqlite's extension API is absent.
        let scratch = scratch("extensions");
        let database = scratch.database.as_ref().unwrap();
        assert_eq!(
            database.connection.load_extension_enable(),
            "rusqlite is built without extension loading"
        );
        // At run time: SQL's load_extension() is refused.
        let error = database
            .connection
            .query_row("SELECT load_extension('libnothing')", [], |row| {
                row.get::<_, rusqlite::types::Value>(0)
            })
            .unwrap_err();
        assert!(error.to_string().contains("not authorized"), "{error}");
    }
}
