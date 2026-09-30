//! A read-only connection to the store, for a separate process that reads a relay's store while
//! the relay runs, and the two window queries it makes.
//!
//! The connection is opened with `SQLITE_OPEN_READ_ONLY` (unlike [`crate::reader::Readers`], which
//! must recover a crashed writer's WAL index and so opens read-write) and `query_only`, and each
//! statement is interrupted after its timeout. It must be opened only while the relay's lock is
//! held: with no writer, opening a WAL store would create its `-wal` and `-shm` files, so
//! [`ReadOnlyStore::open`] refuses a store whose two files are not there. A writer that closes
//! between that check and the open removes both files, and the open's first read creates them
//! anew (empty, and left for the writer's next start, which reuses them): the open then finds
//! them changed and refuses too, having read nothing. A file made anew may take its old inode
//! number, so the files are told apart by their creation time as well; on a file system that
//! keeps none, only the device and inode tell. The two queries select by `height` in one read
//! transaction, each capped at a row count, from a height given or from a number of blocks below
//! the store's own tip, which the same transaction reads: a reader needs no other source for the
//! tip. The node itself never uses this module.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use rusqlite::config::DbConfig;
use rusqlite::types::ToSql;
use rusqlite::{Connection, ErrorCode, OpenFlags, Row};

use crate::connection::{SCHEMA_VERSION, Settings};
use crate::errors::DatabaseError;

/// The most rows either window query returns.
pub const MAX_ROWS: usize = 5_000;

/// The virtual machine steps between two checks of a statement's deadline.
const CHECK_EVERY: i32 = 1000;

/// One row of `missed_blocks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMissed {
    /// The missed slot's first second, in seconds since the network epoch.
    pub timestamp: u32,
    /// The height of the block forged after the missed slots.
    pub height: u32,
    /// The delegate that missed it.
    pub username: String,
}

/// The fields of one `blocks` row the window reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowBlock {
    /// The height.
    pub height: u32,
    /// The slot's first second, in seconds since the network epoch.
    pub timestamp: u32,
    /// The block id.
    pub id: String,
    /// The generator's public key.
    pub generator_public_key: String,
    /// The generator's username, when stored.
    pub username: Option<String>,
}

/// Where a window starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowStart {
    /// At this height.
    Height(u32),
    /// This many blocks below the store's tip, as the same read transaction finds it.
    BelowTip(u32),
}

/// What one read transaction of the window gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRows {
    /// The store's highest height at the read; `None` for a store without blocks.
    pub tip: Option<u32>,
    /// The missed rows of the named usernames from the height on, by height.
    pub missed: Vec<WindowMissed>,
    /// The blocks from the height on, by height.
    pub blocks: Vec<WindowBlock>,
    /// Whether [`MAX_ROWS`] cut either list.
    pub capped: bool,
}

/// Why the store could not be opened or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadOnlyError {
    /// The store's `-wal` or `-shm` file is missing: no writer has it open, and an open would
    /// create them. Nothing was opened.
    #[error("no writer has the store open")]
    NoWriter,
    /// A statement ran past the timeout, or waited that long for a lock, and was given up.
    #[error("the store did not answer within its timeout")]
    TimedOut,
    /// The store could not be opened or read.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

impl From<rusqlite::Error> for ReadOnlyError {
    fn from(error: rusqlite::Error) -> ReadOnlyError {
        match error.sqlite_error_code() {
            Some(ErrorCode::OperationInterrupted | ErrorCode::DatabaseBusy) => {
                ReadOnlyError::TimedOut
            }
            _ => ReadOnlyError::Database(error.into()),
        }
    }
}

/// A read-only connection to a store that a running node writes.
#[derive(Debug)]
pub struct ReadOnlyStore {
    connection: Connection,
    timeout: Duration,
}

/// The device, inode and creation time of a store's `-wal` and `-shm` files, which a writer
/// holds open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterFiles([(u64, u64, Option<Duration>); 2]);

impl WriterFiles {
    /// The `-wal` and `-shm` files of the store at `path` as they stand, when both are regular
    /// files (a link is not followed); `None` otherwise.
    pub fn of(path: &Path) -> Option<WriterFiles> {
        let file = |suffix: &str| {
            let mut beside = path.as_os_str().to_owned();
            beside.push(suffix);
            let meta = std::fs::symlink_metadata(PathBuf::from(beside)).ok()?;
            let created = meta.created().ok();
            let created = created.and_then(|at| at.duration_since(UNIX_EPOCH).ok());
            meta.is_file().then(|| (meta.dev(), meta.ino(), created))
        };
        Some(WriterFiles([file("-wal")?, file("-shm")?]))
    }
}

impl ReadOnlyStore {
    /// Open the store of `settings` read-only, each statement interrupted after `timeout`;
    /// refused with [`ReadOnlyError::NoWriter`], before any open, when its `-wal` or `-shm` file
    /// is missing, and after the open when they are no longer the files seen before it.
    pub fn open(settings: &Settings, timeout: Duration) -> Result<ReadOnlyStore, ReadOnlyError> {
        let seen = WriterFiles::of(&settings.path).ok_or(ReadOnlyError::NoWriter)?;
        ReadOnlyStore::open_seen(settings, timeout, seen)
    }

    /// [`ReadOnlyStore::open`] once its `-wal` and `-shm` were `seen`: the store is opened, and
    /// refused with [`ReadOnlyError::NoWriter`] when after its first read the two files are not
    /// those (a writer that closed meanwhile removed them and the read created them anew).
    pub fn open_seen(
        settings: &Settings,
        timeout: Duration,
        seen: WriterFiles,
    ) -> Result<ReadOnlyStore, ReadOnlyError> {
        // No SQLITE_OPEN_CREATE and no URI file names: the path is an existing file.
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let connection = Connection::open_with_flags(&settings.path, flags)?;
        connection.busy_timeout(timeout)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
        connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
        connection.pragma_update(None, "query_only", true)?;
        let store = ReadOnlyStore {
            connection,
            timeout,
        };
        store.arm()?;
        // The first read opens the `-wal` and `-shm` files, or creates them when they are gone.
        let version: i64 =
            (store.connection).pragma_query_value(None, "user_version", |r| r.get(0))?;
        if WriterFiles::of(&settings.path) != Some(seen) {
            return Err(ReadOnlyError::NoWriter);
        }
        if version != i64::from(SCHEMA_VERSION) {
            let found = version.to_string();
            let expected = SCHEMA_VERSION;
            return Err(DatabaseError::SchemaVersion { found, expected }.into());
        }
        Ok(store)
    }

    /// Interrupt the statements that run from now on once the timeout has passed.
    #[expect(
        clippy::disallowed_types,
        reason = "the deadline bounds how long another process's read runs; no stored value \
                  depends on it"
    )]
    fn arm(&self) -> Result<(), ReadOnlyError> {
        let deadline = std::time::Instant::now().checked_add(self.timeout);
        let expired = move || deadline.is_none_or(|at| std::time::Instant::now() > at);
        let handler = Some(expired);
        Ok(self.connection.progress_handler(CHECK_EVERY, handler)?)
    }

    /// In one read transaction: the store's tip, and the missed rows of `usernames` and the blocks,
    /// both from `start` on, each at most [`MAX_ROWS`].
    pub fn window(
        &self,
        usernames: &[String],
        start: WindowStart,
    ) -> Result<WindowRows, ReadOnlyError> {
        self.arm()?;
        let read = self.connection.unchecked_transaction()?;
        let tip: Option<u32> =
            read.query_row("SELECT MAX(height) FROM blocks", [], |r| r.get(0))?;
        let from_height = match start {
            WindowStart::Height(height) => height,
            WindowStart::BelowTip(blocks) => tip.unwrap_or(0).saturating_sub(blocks),
        };
        // One row more than the cap, to know whether it cut the list.
        let limit = MAX_ROWS as i64 + 1;
        let blocks = "SELECT height, timestamp, id, generator_public_key, username FROM blocks \
                      WHERE height >= ?1 ORDER BY height LIMIT ?2";
        let mut blocks = rows(&read, blocks, &[&from_height, &limit], |row| {
            Ok(WindowBlock {
                height: row.get(0)?,
                timestamp: row.get(1)?,
                id: row.get(2)?,
                generator_public_key: row.get(3)?,
                username: row.get(4)?,
            })
        })?;
        let mut missed = Vec::new();
        if !usernames.is_empty() {
            let marks = vec!["?"; usernames.len()].join(", ");
            let sql = format!(
                "SELECT timestamp, height, username FROM missed_blocks WHERE height >= ? \
                 AND username IN ({marks}) ORDER BY height, timestamp LIMIT ?"
            );
            let mut values: Vec<&dyn ToSql> = vec![&from_height];
            values.extend(usernames.iter().map(|name| name as &dyn ToSql));
            values.push(&limit);
            missed = rows(&read, &sql, &values, |row| {
                Ok(WindowMissed {
                    timestamp: row.get(0)?,
                    height: row.get(1)?,
                    username: row.get(2)?,
                })
            })?;
        }
        let capped = blocks.len() > MAX_ROWS || missed.len() > MAX_ROWS;
        blocks.truncate(MAX_ROWS);
        missed.truncate(MAX_ROWS);
        Ok(WindowRows {
            tip,
            missed,
            blocks,
            capped,
        })
    }
}

/// The rows of `sql` with `values`, each read by `read`.
fn rows<T>(
    connection: &Connection,
    sql: &str,
    values: &[&dyn ToSql],
    read: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, ReadOnlyError> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map(values, read)?;
    Ok(rows.collect::<Result<_, _>>()?)
}
