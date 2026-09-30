//! `heartwood-database`: the store of Heartwood Core, one SQLite file per node.
//!
//! The file is `<data>/database/blockchain.db`. Schema version 1 ([`connection::SCHEMA`]) holds the
//! reference's four chain tables with the reference's columns (`blocks`, `transactions`, `rounds`,
//! `missed_blocks`), the state tables (`wallets`, `state`, `undo`), `meta`, the digests of the
//! applied milestone entries (`milestone_entries`) and the node-local delegate versions
//! (`delegate_versions`). Schema version 2 ([`connection::SCHEMA_2`]) adds the recipients side
//! table (`transaction_recipients`), which the API's address criteria read, and version 3
//! ([`connection::SCHEMA_3`]) the indices of the listing routes' queries; a file of version 1 or 2
//! is upgraded at open.
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
//! - [`recipients`]: the recipients side table's upgrade from schema version 1.
//! - [`reader`]: [`Readers`], the read-only connections of the API, each read interrupted after
//!   3 s; [`history`]: the queries of the served routes on them, with the transaction criteria of
//!   [`filters`] and the two SQL functions they need.
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
pub mod filters;
pub mod history;
pub mod integrity;
pub mod meta;
pub mod missed;
pub mod node_local;
pub mod reader;
pub mod recipients;
pub mod rounds;
pub mod state;
pub mod transactions;
mod values;

pub use crate::blocks::{BlockRow, StoredBlock};
pub use crate::connection::{Chunk, Database, SCHEMA_VERSION, Savepoint, Settings};
pub use crate::errors::DatabaseError;
pub use crate::filters::{
    BlockCriteria, BlockFilter, MissedBlockCriteria, MissedBlockFilter, Numeric,
    TransactionCriteria, TransactionFilter,
};
pub use crate::history::blocks_list::{CountFlag, ListedBlock};
pub use crate::history::download::DownloadBlockRow;
pub use crate::history::fees::{FeeStatisticsRow, avg_int8, round_div};
pub use crate::history::productivity::{ProductivityRow, hundredths, round2};
pub use crate::history::totals::BurnedTotals;
pub use crate::history::{BlockSelector, Direction, Listing, Order, Page, TransactionRecord};
pub use crate::integrity::{IntegrityProblem, IntegrityReport};
pub use crate::meta::{ChainMeta, MilestoneEntryRow};
pub use crate::missed::MissedRow;
pub use crate::node_local::DelegateVersionRow;
pub use crate::reader::{READ_TIMEOUT, Reader, Readers};
pub use crate::rounds::RoundRow;
pub use crate::state::{StateRow, UndoRow, WalletRow};
pub use crate::transactions::TransactionRow;
