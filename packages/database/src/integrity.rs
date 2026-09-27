//! The checks a node runs on its store at every start, before it restores its state.
//!
//! With one transaction per chunk, a store that a crash interrupted holds the old chunk or the
//! new one, never a part; a failed check therefore means a file changed outside the node. The
//! node refuses to start and names the check: there is no automatic rewind.

use heartwood_crypto::Milestones;

use crate::connection::Database;
use crate::errors::DatabaseError;

/// What the checks found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityReport {
    /// The height of the last block, if the store holds any.
    pub tip: Option<u32>,
    /// Every failed check, in the order the checks run.
    pub problems: Vec<IntegrityProblem>,
}

impl IntegrityReport {
    /// Whether every check passed.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// A failed check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityProblem {
    /// The number of blocks is not the tip's height.
    BlockCount {
        /// The tip's height.
        tip: u32,
        /// The number of blocks.
        blocks: u64,
    },
    /// The transaction rows do not add up to a column of the blocks: `number_of_transactions`,
    /// `total_fee` or `total_amount`, as the reference's `verifyBlockchain` compares them.
    TransactionSums {
        /// The block column.
        column: &'static str,
        /// The sum of the column over the blocks.
        blocks: u128,
        /// The count or sum over the transaction rows.
        transactions: u128,
    },
    /// Round rows exist above the round of the tip.
    RoundAbove {
        /// The highest round with rows.
        round: u64,
        /// The round of the tip.
        tip_round: u64,
    },
    /// A height from 2 to the tip has no undo record, or an undo record lies above the tip.
    Undo {
        /// The height.
        height: u32,
    },
    /// The state row is not at the tip. A missing state row is no problem here: the start then
    /// builds the state again from the genesis block.
    StateRow {
        /// The state row's height.
        height: u32,
        /// The tip's height.
        tip: u32,
    },
}

impl Database {
    /// Run the checks, with the round of a height as `milestones` give it.
    pub fn integrity(&self, milestones: &Milestones) -> Result<IntegrityReport, DatabaseError> {
        let _ = milestones;
        todo!("run the boot checks")
    }
}
