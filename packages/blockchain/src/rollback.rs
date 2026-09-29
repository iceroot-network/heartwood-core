//! `snapshot rollback`: the reference's rollback of the stored chain to the last block of a
//! finished round, done on the store in one transaction.
//!
//! The reference (`SnapshotService.rollbackByHeight`) lands on the last block of the round before
//! the requested height's round, strictly below the requested height, and deletes the rows above
//! it: the blocks, their transactions, the missed-slot rows and the round rows above the landing
//! block's round. Its next start rebuilds the wallets from the stored blocks and saves the rows of
//! the round after the landing block again. Here the same rows go, with this store's own rows above
//! the target (the undo records and the milestone entries), and the state tables go with them in
//! the same transaction, so that the next start restores by a replay from the genesis block
//! ([`crate::ReplayReason::NoState`]) and, as the reference's start, writes the rows of the round
//! after the target ([`crate::ChunkWriter::next_round_at_start`]). The pool's storage and the
//! online record are not touched.

use heartwood_crypto::Milestones;
use heartwood_kernel::log::Level;

use crate::commit::ChainStore;
use crate::errors::StoreError;

/// Why a rollback changes nothing, as the reference refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackRefusal {
    /// Neither `--height` nor `--number` was given (or both were 0): "Please specify either a
    /// height or number of blocks to roll back".
    NoRequest,
    /// The height is not above 0: "Rollback height `<h>` is invalid".
    Invalid {
        /// The requested height.
        height: i64,
    },
    /// The height is at or above the stored tip: "Rollback height `<h>` is not less than the
    /// current height `<c>`".
    NotBelowCurrent {
        /// The requested height.
        height: i64,
        /// The stored tip.
        current: u32,
    },
    /// A height of round 1: the block at `round_height - 1` does not exist ("Rollback failed").
    NoBlock {
        /// The first height of the requested height's round.
        round_height: u32,
        /// The stored tip.
        current: u32,
    },
}

impl RollbackRefusal {
    /// The lines the reference logs for the refusal, in order, with their levels.
    /// [`RollbackRefusal::NoRequest`]: the one line (error). [`RollbackRefusal::Invalid`] and
    /// [`RollbackRefusal::NotBelowCurrent`]: "Rolling back the blockchain" (info), then the refusal
    /// (error). [`RollbackRefusal::NoBlock`]: "Rolling back the blockchain" and "Last block height
    /// is: `<c>`" (info), "Rollback failed" (error), then the error's message, "Error: Cannot find
    /// block on height `<round_height>`" (error), which is the first line of the stack the reference
    /// logs.
    pub fn lines(&self) -> Vec<(Level, String)> {
        todo!("the refusal's lines")
    }
}

/// A rollback the reference would make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackPlan {
    /// The stored tip.
    pub current: u32,
    /// The requested height (`--height`, or the tip minus `--number`).
    pub requested: i64,
    /// The requested height's round.
    pub round: u64,
    /// The landing block: the first height of that round minus 1.
    pub target: u32,
}

/// The plan of a rollback of a chain whose tip is `current`: `--height` when given and not 0,
/// else `--number` when given and not 0 (the tip minus it), else [`RollbackRefusal::NoRequest`],
/// as the reference's command reads its flags.
pub fn plan(
    height: Option<i64>,
    number: Option<i64>,
    current: u32,
    milestones: &Milestones,
) -> Result<RollbackPlan, RollbackRefusal> {
    let _ = (height, number, current, milestones);
    todo!("the reference's target and refusals")
}

/// What a rollback deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rolled {
    /// The tip before.
    pub from: u32,
    /// The tip after: the plan's target.
    pub target: u32,
    /// The requested height's round, which the last line names.
    pub round: u64,
    /// The blocks deleted.
    pub blocks: u32,
    /// The transactions deleted.
    pub transactions: u64,
    /// The missed-slot rows deleted.
    pub missed: u64,
    /// The round rows deleted.
    pub round_rows: u64,
}

/// Roll the store back as `plan` says, in one transaction: the rows above the target
/// (`delete_above(target, the target's round)`) and the state tables, then the commit.
pub fn rollback(
    store: &mut ChainStore,
    plan: &RollbackPlan,
    milestones: &Milestones,
) -> Result<Rolled, StoreError> {
    let _ = (store, plan, milestones);
    todo!("the store change in one transaction")
}

/// The three info lines of a rollback that was made: "Rolling back the blockchain", "Last block
/// height is: `<c>`", "Rolling back blockchain to last finished round `<r>` with last block height
/// `<h>`".
pub fn lines(plan: &RollbackPlan, rolled: &Rolled) -> [String; 3] {
    let _ = (plan, rolled);
    todo!("the rollback's lines")
}
