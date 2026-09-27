//! The checks a node runs on its store at every start, before it restores its state.
//!
//! With one transaction per chunk, a store that a crash interrupted holds the old chunk or the
//! new one, never a part; a failed check therefore means a file changed outside the node. The
//! node refuses to start and names the check: there is no automatic rewind.

use heartwood_crypto::Milestones;
use heartwood_kernel::calculate_round;
use rusqlite::OptionalExtension;

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
    /// Round rows exist above the last round the tip may have rows of: the round of the block
    /// after the tip, which is the tip's own round, or the next one when the tip ends its round
    /// (that block saved the next round's rows).
    RoundAbove {
        /// The highest round with rows.
        round: u64,
        /// The last round the tip may have rows of (0 when the store holds no block).
        tip_round: u64,
    },
    /// A height from 2 to the tip has no undo record (the lowest such height), or an undo record
    /// lies outside those heights (the lowest such record).
    Undo {
        /// The height.
        height: u32,
    },
    /// The state row is not at the tip. A missing state row is no problem here: the start then
    /// builds the state again from the genesis block.
    StateRow {
        /// The state row's height.
        height: u32,
        /// The tip's height (0 when the store holds no block).
        tip: u32,
    },
    /// The state row is at the tip's height but names another block than the tip.
    StateBlock {
        /// The tip's height.
        height: u32,
    },
}

impl Database {
    /// Run the checks, with the round of a height as `milestones` give it.
    pub fn integrity(&self, milestones: &Milestones) -> Result<IntegrityReport, DatabaseError> {
        let connection = &self.connection;
        let mut problems = Vec::new();

        let (tip, blocks): (Option<u32>, i64) =
            connection.query_row("SELECT MAX(height), COUNT(*) FROM blocks", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        let height = tip.unwrap_or(0);
        if i64::from(height) != blocks {
            problems.push(IntegrityProblem::BlockCount {
                tip: height,
                blocks: u64::try_from(blocks).unwrap_or(0),
            });
        }

        // Each amount is summed as its high and low 32 bits, which cannot overflow SQLite's
        // 64-bit sums, and put together in 128 bits.
        let sums = |sql: &str| -> Result<[i128; 3], DatabaseError> {
            let parts: [i64; 5] = connection.query_row(sql, [], |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ])
            })?;
            let [count, fee_high, fee_low, amount_high, amount_low] = parts.map(i128::from);
            Ok([
                count,
                (fee_high << 32) + fee_low,
                (amount_high << 32) + amount_low,
            ])
        };
        let block_sums = sums(
            "SELECT COALESCE(SUM(number_of_transactions), 0), \
             COALESCE(SUM(total_fee >> 32), 0), COALESCE(SUM(total_fee & 4294967295), 0), \
             COALESCE(SUM(total_amount >> 32), 0), COALESCE(SUM(total_amount & 4294967295), 0) \
             FROM blocks",
        )?;
        let transaction_sums = sums(
            "SELECT COUNT(*), \
             COALESCE(SUM(fee >> 32), 0), COALESCE(SUM(fee & 4294967295), 0), \
             COALESCE(SUM(amount >> 32), 0), COALESCE(SUM(amount & 4294967295), 0) \
             FROM transactions",
        )?;
        for (column, blocks, transactions) in [
            ("number_of_transactions", block_sums[0], transaction_sums[0]),
            ("total_fee", block_sums[1], transaction_sums[1]),
            ("total_amount", block_sums[2], transaction_sums[2]),
        ] {
            if blocks != transactions {
                let unsigned = |sum: i128, table: &'static str| {
                    u128::try_from(sum).map_err(|_| DatabaseError::CorruptRow {
                        table,
                        detail: format!("the sum behind {column} is negative"),
                    })
                };
                problems.push(IntegrityProblem::TransactionSums {
                    column,
                    blocks: unsigned(blocks, "blocks")?,
                    transactions: unsigned(transactions, "transactions")?,
                });
            }
        }

        let tip_round = match tip {
            Some(height) => calculate_round(height, milestones).next_round,
            None => 0,
        };
        let round: Option<i64> =
            connection.query_row("SELECT MAX(round) FROM rounds", [], |row| row.get(0))?;
        if let Some(round) = round.and_then(|round| u64::try_from(round).ok())
            && round > tip_round
        {
            problems.push(IntegrityProblem::RoundAbove { round, tip_round });
        }

        let outside: Option<u32> = connection.query_row(
            "SELECT MIN(height) FROM undo WHERE height < 2 OR height > ?1",
            [height],
            |row| row.get(0),
        )?;
        let covered: u32 = connection.query_row(
            "SELECT COUNT(*) FROM undo WHERE height BETWEEN 2 AND ?1",
            [height],
            |row| row.get(0),
        )?;
        if covered < height.saturating_sub(1) {
            // The lowest height from 2 whose undo record is missing: 2, or one above a record
            // whose next height has none.
            let missing: Option<u32> = connection.query_row(
                "SELECT MIN(h) FROM ( \
                   SELECT 2 AS h WHERE NOT EXISTS (SELECT 1 FROM undo WHERE height = 2) \
                   UNION ALL \
                   SELECT u.height + 1 FROM undo u WHERE u.height BETWEEN 2 AND ?1 - 1 \
                     AND NOT EXISTS (SELECT 1 FROM undo v WHERE v.height = u.height + 1))",
                [height],
                |row| row.get(0),
            )?;
            if let Some(missing) = missing {
                problems.push(IntegrityProblem::Undo { height: missing });
            }
        }
        if let Some(outside) = outside {
            problems.push(IntegrityProblem::Undo { height: outside });
        }

        if let Some(state) = self.state_row()? {
            if state.height != height {
                problems.push(IntegrityProblem::StateRow {
                    height: state.height,
                    tip: height,
                });
            } else {
                let id: Option<String> = connection
                    .query_row("SELECT id FROM blocks WHERE height = ?1", [height], |row| {
                        row.get(0)
                    })
                    .optional()?;
                if id.as_deref() != Some(state.block_id.to_hex().as_str()) {
                    problems.push(IntegrityProblem::StateBlock { height });
                }
            }
        }

        Ok(IntegrityReport { tip, problems })
    }
}
