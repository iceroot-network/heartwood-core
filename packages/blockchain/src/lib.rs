//! `heartwood-blockchain`: the blockchain component of Heartwood Core.
//!
//! It stores the chain and restores the state from it, processes blocks as the reference's
//! processor does, and runs the follower: the node thread that owns the state, the store and the
//! pool, the job queue, the state machine and the blockchain service around them.
//!
//! Storage and the start:
//!
//! - [`commit`]: [`ChainStore`] and [`ChunkWriter`]: an applied or reverted block's rows, its
//!   changed wallets, its undo record, the state row, the round and missed rows and the applied
//!   milestone entries, all in the chunk's one transaction.
//! - [`rows`]: a block and its transactions as the store's rows, with the columns the reference
//!   derives, and a stored block back as a block.
//! - [`restore`](mod@restore): [`restore()`], a node's start: the first start from the genesis
//!   block, the boot checks, the state read back from the state tables, or replayed from the
//!   genesis block.
//! - [`milestones`]: the digests of the applied milestone entries, and the check at start that no
//!   applied entry changed.
//! - [`online`]: the pool's node-local online record in the store, saved when it changes and
//!   loaded at every start.
//!
//! Blocks and the runtime:
//!
//! - [`processor`]: the block processor, its checks in the reference's order and its outcomes,
//!   behind the [`BlockProcess`] seam.
//! - [`process_blocks_job`]: a job of blocks, committed in one SQLite transaction.
//! - [`node`]: the node thread; [`work`]: its ordered work, its handle and published snapshots,
//!   and the jobs' blocks and outcomes; [`pool_task`]: the pool on it; [`events`]: its events and
//!   listeners.
//! - [`queue`]: the serial job queue.
//! - [`blockchain`]: the blockchain service; [`state_machine`]: the reference's machine and its
//!   actions; [`fork_recovery`]: the recovery's depth.
//! - [`chain_state`]: what the reference keeps in its state store besides the state.
//! - [`network`]: the network monitor's calls, as the [`Network`] trait.
//! - [`options`]: the blockchain options.
//! - [`errors`]: the error types, [`StoreError`], [`StartError`], [`NodeError`] and
//!   [`NodeStopped`].
//!
//! A [`StoreError`] is fatal: the state in memory is then ahead of the store, so the node stops,
//! and its next start restores the state from the store.
//!
//! Features: `difftest` adds the seeded and fixed rollback depths of the fork recovery, test seams
//! that only test code enables; a release build never has them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation
)]

pub mod blockchain;
pub mod chain_state;
pub mod commit;
pub mod errors;
pub mod events;
pub mod fork_recovery;
pub mod milestones;
pub mod network;
pub mod node;
pub mod online;
pub mod options;
pub mod pool_task;
pub mod process_blocks_job;
pub mod processor;
pub mod queue;
pub mod restore;
pub mod rows;
pub mod state_machine;
pub mod work;

pub use crate::blockchain::{Blockchain, BlockchainParts, MachineStatus};
pub use crate::chain_state::{ChainState, SharedChainState};
pub use crate::commit::{ChainStore, ChunkWriter};
pub use crate::errors::{NodeError, NodeStopped, StartError, StoreError};
pub use crate::fork_recovery::RollbackDepth;
pub use crate::milestones::entry_digests;
pub use crate::network::Network;
pub use crate::node::{Node, NodeParts};
pub use crate::options::BlockchainOptions;
pub use crate::pool_task::PoolTask;
pub use crate::process_blocks_job::{JobContext, run_job};
pub use crate::processor::{
    BlockProcess, BlockProcessor, BlockProcessorResult, ProcessContext, Processed, UnchainedStatus,
};
pub use crate::queue::JobQueue;
pub use crate::restore::{ReplayReason, RestoreMode, RestorePath, Restored, restore};
pub use crate::work::{
    ChainStatus, Channel, IncomingBlock, Job, JobEnd, JobOutcome, NodeHandle, OnlineWork, Work,
};
