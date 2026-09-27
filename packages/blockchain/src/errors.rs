//! Error types of the crate.

use heartwood_database::{DatabaseError, IntegrityProblem};
use heartwood_state::errors::{ApplyError, GenesisError};
use thiserror::Error;

/// The store could not be written or read. Fatal: the node stops, and its next start restores
/// the state from the store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StoreError {
    /// The database failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// A record could not be written as text.
    #[error("cannot write the {what} record: {detail}")]
    Encode {
        /// What was written.
        what: &'static str,
        /// Why.
        detail: String,
    },
    /// A stored row does not read back as what was written: a record, a block or a
    /// transaction.
    #[error("the stored {what} does not read back: {detail}")]
    Decode {
        /// What was read.
        what: &'static str,
        /// Why.
        detail: String,
    },
    /// A value of a block does not fit its column.
    #[error("{what} of the block at height {height} does not fit its column")]
    Range {
        /// The value.
        what: &'static str,
        /// The block's height.
        height: u32,
    },
    /// The milestone file's entries cannot be read to compute their digests.
    #[error("the milestone entries cannot be read: {0}")]
    Milestones(String),
}

/// The node refuses to start. The message names the failed check or what the operator can do.
/// `E` is the handlers' refusal (`heartwood_state::TransactionHandlers::Error`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StartError<E> {
    /// The store could not be read or written.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A boot check of the store failed.
    #[error("the database failed a check at start: {0:?}")]
    Integrity(IntegrityProblem),
    /// The store belongs to another chain than the configured files: its nethash or its genesis
    /// block differs.
    #[error(
        "the database belongs to another chain: its {what} is {stored}, the configuration's is {configured}"
    )]
    OtherChain {
        /// `nethash` or `genesis block`.
        what: &'static str,
        /// The stored value.
        stored: String,
        /// The configured value.
        configured: String,
    },
    /// The configured genesis block is refused.
    #[error(transparent)]
    Genesis(GenesisError<E>),
    /// An applied milestone entry changed, disappeared, or a new entry lies at or below the tip.
    #[error(
        "the milestone at height {height} changed after it was applied; roll back to height {} or resync",
        height.saturating_sub(1)
    )]
    MilestoneChanged {
        /// The entry's height.
        height: u32,
    },
    /// Replaying the stored chain would take a balance below zero.
    #[error("State Generation failed: Wallet with negative balance")]
    NegativeBalance {
        /// The height of the block that would.
        height: u32,
    },
    /// Replaying the stored chain failed otherwise.
    #[error("State Generation failed: {error}")]
    Replay {
        /// The height of the block that failed.
        height: u32,
        /// Why.
        error: ApplyError<E>,
    },
}
