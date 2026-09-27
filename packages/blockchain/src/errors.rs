//! Error types of the crate.

use heartwood_database::{DatabaseError, IntegrityProblem};
use heartwood_pool::StorageError;
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
    /// A chunk was started before the store was started ([`crate::restore()`]).
    #[error("the store has not been started: restore the state from it first")]
    NotStarted,
}

/// The node refuses to start. The message names the failed check or what the operator can do.
/// `E` is the handlers' refusal (`heartwood_state::TransactionHandlers::Error`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StartError<E> {
    /// The store could not be read or written.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A boot check of the store failed.
    #[error("the database failed a check at start: {0}")]
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
    /// The configured genesis block's JSON does not decode.
    #[error("the genesis block could not be decoded: {detail}")]
    GenesisDecode {
        /// Why.
        detail: String,
    },
    /// The store holds blocks without the chain's identity, or the identity without blocks: a
    /// first start writes both in one transaction, so the file was changed outside the node.
    #[error("the database's chain identity is incomplete: {detail}")]
    Identity {
        /// What is missing.
        detail: &'static str,
    },
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

impl<E> From<DatabaseError> for StartError<E> {
    fn from(error: DatabaseError) -> StartError<E> {
        StartError::Store(StoreError::Database(error))
    }
}

/// The node thread stopped, or has stopped: a request to it gets no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the node has stopped")]
pub struct NodeStopped;

/// A fatal error of the node thread: the node stops (the reference exits with 1), and its next
/// start restores the committed tip.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NodeError {
    /// The store could not be written or read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The pool's storage could not be written or read.
    #[error("the pool's storage failed: {0}")]
    Pool(#[from] StorageError),
    /// A block of a removal could not be reverted.
    #[error("the block at height {height} could not be reverted: {reason}")]
    Revert {
        /// The block's height.
        height: u32,
        /// Why.
        reason: String,
    },
    /// The processor reported a corrupted state.
    #[error("the block at height {height} left the state corrupted")]
    Corrupted {
        /// The block's height.
        height: u32,
    },
    /// The stored tip is not the state's tip after a removal.
    #[error("the stored tip is at height {stored}, the state's at {state}")]
    TipMismatch {
        /// The stored tip's height.
        stored: u32,
        /// The state's tip's height.
        state: u32,
    },
    /// The node thread had stopped.
    #[error("the node has stopped")]
    Stopped,
}

impl From<NodeStopped> for NodeError {
    fn from(_: NodeStopped) -> NodeError {
        NodeError::Stopped
    }
}
