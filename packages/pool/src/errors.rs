//! Error types of the crate.
//!
//! A transaction the pool does not take is a [`Refusal`]: the reference's error code and message,
//! which the processor reports for its entry. A failure of `pool.sqlite` is a [`StorageError`],
//! which is fatal for the node, as a failed write of the reference's storage throws out of the
//! pool.

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// The reference's error codes of the pool.
///
/// The reference's `ERR_EXPIRED` has no variant: none of the kept transaction types carries an
/// expiration, so no transaction expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolCode {
    /// `ERR_DUPLICATE`: the transaction is already in the pool.
    Duplicate,
    /// `ERR_COOLDOWN`: the processor saw the transaction less than 30 seconds ago.
    Cooldown,
    /// `ERR_TOO_LARGE`: the transaction is longer than `maxTransactionBytes`.
    TooLarge,
    /// `ERR_WRONG_NETWORK`: the transaction's network byte is not the node's.
    WrongNetwork,
    /// `ERR_LOW_FEE`: the fee is below the pool's floor, or below the static fee.
    LowFee,
    /// `ERR_HIGH_FEE`: the fee is above the static fee (static-fee mode only).
    HighFee,
    /// `ERR_POOL_FULL`: the pool is full and the fee is not above the lowest.
    PoolFull,
    /// `ERR_EXCEEDS_MAX_COUNT`: the sender has `maxTransactionsPerSender` transactions in the
    /// pool.
    ExceedsMaxCount,
    /// `ERR_APPLY`: the transaction cannot be applied to its sender's view, or may not enter the
    /// pool (pending and offline checks included).
    Apply,
    /// `ERR_BAD_DATA`: the entry does not decode, or its signature does not verify.
    BadData,
    /// `ERR_RETRY`: the sender's view was left inconsistent by a failed revert.
    Retry,
    /// `ERR_OTHER`: any other refusal, such as a transaction of a removed type.
    Other,
}

impl PoolCode {
    /// The code as the reference writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            PoolCode::Duplicate => "ERR_DUPLICATE",
            PoolCode::Cooldown => "ERR_COOLDOWN",
            PoolCode::TooLarge => "ERR_TOO_LARGE",
            PoolCode::WrongNetwork => "ERR_WRONG_NETWORK",
            PoolCode::LowFee => "ERR_LOW_FEE",
            PoolCode::HighFee => "ERR_HIGH_FEE",
            PoolCode::PoolFull => "ERR_POOL_FULL",
            PoolCode::ExceedsMaxCount => "ERR_EXCEEDS_MAX_COUNT",
            PoolCode::Apply => "ERR_APPLY",
            PoolCode::BadData => "ERR_BAD_DATA",
            PoolCode::Retry => "ERR_RETRY",
            PoolCode::Other => "ERR_OTHER",
        }
    }
}

impl fmt::Display for PoolCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A transaction the pool does not take: the reference's code and message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct Refusal {
    /// The code.
    pub code: PoolCode,
    /// The message, exactly as the reference writes it.
    pub message: String,
}

impl Refusal {
    /// A refusal with `code` and `message`.
    pub fn new(code: PoolCode, message: impl Into<String>) -> Refusal {
        Refusal {
            code,
            message: message.into(),
        }
    }
}

/// Why a transaction was not added to the pool.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AddError {
    /// The pool refused it.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The storage failed (fatal).
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// `pool.sqlite` could not be opened, written or read. Fatal for the node.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StorageError {
    /// SQLite could not open the file or refused a statement.
    #[error("pool storage: SQLite: {0}")]
    Sqlite(String),
    /// A file of the pool's storage could not be created or removed.
    #[error("pool storage: {}: {detail}", path.display())]
    File {
        /// The file or directory.
        path: PathBuf,
        /// Why.
        detail: String,
    },
    /// A stored row cannot be read back.
    #[error("pool storage: a stored row cannot be read: {0}")]
    CorruptRow(String),
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> StorageError {
        StorageError::Sqlite(error.to_string())
    }
}
