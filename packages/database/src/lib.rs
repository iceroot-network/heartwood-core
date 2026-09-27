//! `heartwood-database`: the store of Heartwood Core, one SQLite file per node.
//!
//! The file is `<data>/database/blockchain.db`. Schema version 1 ([`connection::SCHEMA`]) holds the
//! reference's four chain tables with the reference's columns (`blocks`, `transactions`, `rounds`,
//! `missed_blocks`), the state tables (`wallets`, `state`, `undo`), `meta`, the digests of the
//! applied milestone entries (`milestone_entries`) and the node-local delegate versions
//! (`delegate_versions`).
//!
//! - [`connection`]: [`Database`], the one writer connection, and [`Chunk`], one SQLite
//!   transaction per chunk of blocks with a [`Savepoint`] per block.
//! - [`blocks`], [`transactions`]: block and transaction rows.
//! - [`rounds`], [`missed`]: round rows and missed-slot rows.
//! - [`state`]: the wallet, state and undo rows, whose records are text that `heartwood-state`
//!   writes and reads.
//! - [`meta`]: the chain's identity and the applied milestone entries.
//! - [`node_local`]: the delegate versions, which never enter consensus.
//! - [`integrity`]: the checks a node runs on every start.
//! - [`errors`]: the crate's error type, [`DatabaseError`].
//!
//! Rules: the crate stores rows, never state types: it does not depend on `heartwood-state`, and
//! `heartwood-blockchain` maps the state to rows. Ids, keys and hashes are lowercase hex text.
//! Amounts are `INTEGER` columns written through a checked conversion to `i64`; a value outside
//! `i64` is [`DatabaseError::Range`], a fatal storage error. SQLite extensions are never loaded.

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

pub mod blocks;
pub mod connection;
pub mod errors;
pub mod integrity;
pub mod meta;
pub mod missed;
pub mod node_local;
pub mod rounds;
pub mod state;
pub mod transactions;

pub use crate::blocks::{BlockRow, StoredBlock};
pub use crate::connection::{Chunk, Database, SCHEMA_VERSION, Savepoint, Settings};
pub use crate::errors::DatabaseError;
pub use crate::integrity::{IntegrityProblem, IntegrityReport};
pub use crate::meta::{ChainMeta, MilestoneEntryRow};
pub use crate::missed::MissedRow;
pub use crate::node_local::DelegateVersionRow;
pub use crate::rounds::RoundRow;
pub use crate::state::{StateRow, UndoRow, WalletRow};
pub use crate::transactions::TransactionRow;
