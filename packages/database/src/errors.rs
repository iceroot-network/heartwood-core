//! Error types of the crate.

use std::path::PathBuf;

use thiserror::Error;

/// A read or a write of the database failed.
///
/// Every one is fatal for a node: after a failed commit the state in memory is ahead of the
/// store, so the node stops, and its next start restores from the store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DatabaseError {
    /// SQLite could not open the file or refused a statement.
    #[error("SQLite: {0}")]
    Sqlite(String),
    /// The directory of the database file could not be created.
    #[error("cannot create {}: {detail}", path.display())]
    Directory {
        /// The directory.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// A value does not fit its column: amounts are written as signed 64-bit integers, where the
    /// reference's `bigint` columns stop too.
    #[error("{column}: {value} does not fit a signed 64-bit column")]
    Range {
        /// The table and column, as `table.column`.
        column: &'static str,
        /// The value.
        value: u128,
    },
    /// A stored row cannot be read back: a value has the wrong type or is out of its range, or an
    /// id, key or hash is not hex of its length.
    #[error("{table}: a stored row cannot be read: {detail}")]
    CorruptRow {
        /// The table.
        table: &'static str,
        /// What is wrong.
        detail: String,
    },
    /// The file holds another schema version than this build reads, or is not a database of a
    /// node.
    #[error("the database has schema version {found}; this node reads version {expected}")]
    SchemaVersion {
        /// The version the file names, or a description of what was found instead.
        found: String,
        /// The version this build reads.
        expected: u32,
    },
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(error: rusqlite::Error) -> DatabaseError {
        DatabaseError::Sqlite(error.to_string())
    }
}
