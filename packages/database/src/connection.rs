//! The writer connection, the schema, and chunks.
//!
//! A node has one [`Database`], which owns the one writer connection to its file, opened in WAL
//! mode with `synchronous = FULL`, `foreign_keys = ON`, `busy_timeout = 5000` and `mmap_size = 0`.
//! An empty file gets schema version 1 ([`SCHEMA`]); a file of another version is refused.
//!
//! Everything a chunk of blocks changes is written in one SQLite transaction, a [`Chunk`]: its
//! rows are all stored, or, after a crash or when the chunk is dropped, none. Each block of the
//! chunk has its own [`Savepoint`], so that a block refused halfway leaves the chunk's other
//! blocks.

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use crate::errors::DatabaseError;

/// The schema version this build creates and reads.
pub const SCHEMA_VERSION: u32 = 1;

/// Schema version 1: every table and index, as SQL statements.
pub const SCHEMA: &str = include_str!("schema.sql");

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
    #[expect(dead_code, reason = "the methods of the store read it")]
    connection: rusqlite::Connection,
}

impl Database {
    /// Open the database of `settings`, creating the file, its directory and schema version 1
    /// when the file does not exist or is empty. A file of another schema version is refused
    /// ([`DatabaseError::SchemaVersion`]).
    pub fn open(settings: &Settings) -> Result<Database, DatabaseError> {
        let _ = settings;
        todo!("open the writer connection and create or check the schema")
    }

    /// Start a chunk: one SQLite transaction for everything the chunk's blocks change. Dropping
    /// the chunk without [`Chunk::commit`] rolls every change back.
    pub fn begin(&mut self) -> Result<Chunk<'_>, DatabaseError> {
        todo!("begin a transaction")
    }

    /// Delete every row of every table, in one transaction: the store of a node that starts again
    /// from its genesis block, whose first start writes every table again, `meta` included.
    pub fn reset(&mut self) -> Result<(), DatabaseError> {
        todo!("delete every row")
    }
}

/// One chunk of blocks: a transaction on the writer connection. Its rows become visible, all at
/// once, when it is committed; dropping it rolls them back.
#[derive(Debug)]
pub struct Chunk<'d> {
    #[expect(dead_code, reason = "the methods of the store read it")]
    transaction: rusqlite::Transaction<'d>,
}

impl<'d> Chunk<'d> {
    /// Start one block's savepoint. The block's writes go through it; committing it keeps them in
    /// the chunk, and dropping it (a refused block) rolls back to the state of the chunk before
    /// the block.
    pub fn savepoint(&mut self) -> Result<Savepoint<'_, 'd>, DatabaseError> {
        todo!("start a savepoint")
    }

    /// Commit the chunk: every row it wrote is stored, durably, before this returns.
    pub fn commit(self) -> Result<(), DatabaseError> {
        todo!("commit the transaction")
    }
}

/// One block's savepoint inside a [`Chunk`]. It gives access to the chunk's writes; dropping it
/// without [`Savepoint::commit`] rolls back what was written since it started.
#[derive(Debug)]
pub struct Savepoint<'c, 'd> {
    chunk: &'c mut Chunk<'d>,
}

impl Savepoint<'_, '_> {
    /// Keep the block's writes in the chunk.
    pub fn commit(self) -> Result<(), DatabaseError> {
        todo!("release the savepoint")
    }

    /// Roll back what was written since the savepoint started.
    pub fn rollback(self) -> Result<(), DatabaseError> {
        todo!("roll back to the savepoint")
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
