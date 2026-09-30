//! `heartwood-pool`: the transaction pool of Heartwood Core.
//!
//! The pool admits transactions as the reference's pool does: the same checks in the same
//! order, the same error codes and messages, the same lists in a processor result, and the same
//! orders when it is queried and collated. Every admission, re-add and collation runs under the
//! rules of the next block (the tip plus one); the collator's byte budget is the tip's.
//!
//! - [`options`]: [`Options`], the pool's settings with the reference's defaults.
//! - [`storage`]: `pool.sqlite`, the admitted transactions in the reference's table, and the
//!   clear marker.
//! - [`mempool`], [`sender_mempool`], [`sender_state`]: every sender's pending transactions, in
//!   arrival order, each sender with its own view of the wallets.
//! - [`processor`]: [`Processor`], which decodes a batch of entries and adds them, with its own
//!   30-second cooldown.
//! - [`dynamic_fee_matcher`]: the fee rules to enter the pool and to be broadcast, dynamic or
//!   static.
//! - [`query`]: [`Query`], the pool's transactions in the reference's orders.
//! - [`service`]: [`Pool`]: add, re-add, remove, remove forged, clean up and flush.
//! - [`collator`]: [`collate`], the transactions of a block candidate.
//! - [`verifier`]: decoding and signature checks of a batch on several threads.
//! - [`online`]: [`DelegateVersions`], the node-local record of the delegates that operate a
//!   node.
//! - [`view`]: [`PoolView`], the view of the pool and the online record that the node publishes.
//! - [`errors`]: the error types, [`Refusal`] with its [`PoolCode`], [`AddError`] and
//!   [`StorageError`].
//!
//! The pool is synchronous: every operation takes `&mut self` and returns what it added and
//! removed; the node runs it in a task of its own and turns the results into events. It reads
//! the time only through [`heartwood_kernel::clock::Clock`].

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

pub mod collator;
pub mod dynamic_fee_matcher;
pub mod errors;
pub mod mempool;
pub mod online;
pub mod options;
pub mod processor;
pub mod query;
pub mod sender_mempool;
pub mod sender_state;
pub mod service;
pub mod storage;
pub mod verifier;
pub mod view;

pub use crate::collator::{Collated, collate};
pub use crate::errors::{AddError, PoolCode, Refusal, StorageError};
pub use crate::online::{DelegateVersions, OnlineEntry, OnlineView};
pub use crate::options::{Options, PoolFees};
pub use crate::processor::{Entry, Processor, ProcessorResult};
pub use crate::query::Query;
pub use crate::service::{Pool, PoolContext, Readded, Reindexed, Removed};
pub use crate::storage::{Storage, StoredTransaction};
pub use crate::verifier::Checked;
pub use crate::view::{PoolView, PoolViewOptions};
