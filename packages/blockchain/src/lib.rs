//! `heartwood-blockchain`: the blockchain component of Heartwood Core.
//!
//! This part stores the chain and restores the state from it; the block processor, the queue and
//! the state machine that drive it come later.
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
//! - [`errors`]: the error types, [`StoreError`] and [`StartError`].
//!
//! A [`StoreError`] is fatal: the state in memory is then ahead of the store, so the node stops,
//! and its next start restores the state from the store.

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

pub mod commit;
pub mod errors;
pub mod milestones;
pub mod online;
pub mod restore;
pub mod rows;

pub use crate::commit::{ChainStore, ChunkWriter};
pub use crate::errors::{StartError, StoreError};
pub use crate::milestones::entry_digests;
pub use crate::restore::{ReplayReason, RestoreMode, RestorePath, Restored, restore};
