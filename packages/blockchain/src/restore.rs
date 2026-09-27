//! A node's start: the state restored from the store.
//!
//! 1. An empty store is a first start: the state after the configured genesis block, written
//!    with the chain's identity in one transaction.
//! 2. Otherwise the boot checks run on every start: the store names the configured nethash and
//!    genesis block, the configured genesis block passes its checks and is the stored one, and
//!    the store's own checks pass ([`heartwood_database::Database::integrity`]). A failed check
//!    refuses the start and names the check; nothing is rewound.
//! 3. The applied milestone entries must be unchanged ([`crate::milestones`]).
//! 4. With [`RestoreMode::Auto`] and a state row of the current format, the state is read back
//!    from the state tables and checked ([`heartwood_state::State::restore`]). A missing,
//!    older-format or inconsistent state is no error: the start replays instead.
//! 5. The replay: the state after the genesis block, then every stored block applied again
//!    through the one apply path; then the state tables are written again. A replay that would
//!    take a balance below zero refuses the start with the reference's message.

use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_state::persist::RestoreError;
use heartwood_state::{State, TransactionHandlers};

use crate::commit::ChainStore;
use crate::errors::StartError;

/// How the state is to be restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    /// From the state tables when they hold a consistent state, else by a replay.
    Auto,
    /// By a replay from the genesis block, as a `state clear` asks.
    Replay,
}

/// Why a start replayed the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayReason {
    /// [`RestoreMode::Replay`] asked for it.
    Requested,
    /// The store has no state row.
    NoState,
    /// The state row's format is not the current one.
    Format {
        /// The stored format.
        found: u32,
    },
    /// The stored state failed a check.
    Inconsistent(RestoreError),
}

/// How the state was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePath {
    /// A first start, from the genesis block.
    FirstStart,
    /// From the state tables.
    Table,
    /// By a replay from the genesis block.
    Replay(ReplayReason),
}

/// A restored state, and how it was restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The state at the stored tip.
    pub state: State,
    /// How it was restored.
    pub path: RestorePath,
}

/// Restore the state of `store` under the configuration `files`, with `handlers` for the genesis
/// block and a replay.
pub fn restore<H: TransactionHandlers>(
    store: &mut ChainStore,
    files: &CryptoFiles,
    handlers: &H,
    mode: RestoreMode,
) -> Result<Restored, StartError<H::Error>> {
    let _ = (store, files, handlers, mode);
    todo!("restore the state")
}
