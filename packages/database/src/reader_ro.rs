//! A read-only connection to the store, for a separate process that reads a relay's store while
//! the relay runs, and the two window queries it makes.
//!
//! The connection is opened with `SQLITE_OPEN_READ_ONLY` (unlike [`crate::reader::Readers`], which
//! must recover a crashed writer's WAL index and so opens read-write) and `query_only`, and each
//! statement is interrupted after its timeout. It must be opened only while the relay's lock is
//! held: with no writer, opening a WAL store would create its `-wal` and `-shm` files. The two
//! queries select by `height` in one read transaction, each capped at a row count. The node itself
//! never uses this module.

use std::time::Duration;

use rusqlite::Connection;

use crate::connection::Settings;
use crate::errors::DatabaseError;

/// The most rows either window query returns.
pub const MAX_ROWS: usize = 5_000;

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

/// What one read transaction of the window gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRows {
    /// The missed rows of the named usernames from the height on, by height.
    pub missed: Vec<WindowMissed>,
    /// The blocks from the height on, by height.
    pub blocks: Vec<WindowBlock>,
    /// Whether [`MAX_ROWS`] cut either list.
    pub capped: bool,
}

/// A read-only connection to a store that a running node writes.
#[derive(Debug)]
pub struct ReadOnlyStore {
    connection: Option<Connection>,
}

impl ReadOnlyStore {
    /// Open the store of `settings` read-only, each statement interrupted after `timeout`.
    pub fn open(settings: &Settings, timeout: Duration) -> Result<ReadOnlyStore, DatabaseError> {
        let _ = (settings, timeout);
        Err(DatabaseError::Sqlite(
            "the read-only opener is not built".into(),
        ))
    }

    /// In one read transaction: the missed rows of `usernames` and the blocks, both from
    /// `from_height` on, each at most [`MAX_ROWS`].
    pub fn window(
        &self,
        usernames: &[String],
        from_height: u32,
    ) -> Result<WindowRows, DatabaseError> {
        let _ = (&self.connection, usernames, from_height);
        Err(DatabaseError::Sqlite(
            "the read-only opener is not built".into(),
        ))
    }
}
