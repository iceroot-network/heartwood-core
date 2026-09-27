//! A block and its transactions as the store's rows, and a stored block back as a block.
//!
//! The derived columns are those the reference computes: a block's `burned_fee` under the
//! milestone at its height, its `donations` as the reference's object (every entry the block
//! carries, in the milestone's order), the generator's `username` (none at height 1); a
//! transaction's `sequence` in block order, the block's `timestamp`, the derived `sender_id`, the
//! `recipient_id` where the reference sets one, the top-level `amount`, the transaction's
//! `burned_fee`, the bytes the block carries after it was received, and the `asset` as the
//! reference's JSON.

use heartwood_crypto::{Block, Milestones};
use heartwood_database::{BlockRow, MissedRow, RoundRow, StoredBlock, TransactionRow};
use heartwood_state::round_state::{MissedBlock, RoundDelegate};

use crate::errors::StoreError;

/// The row of `block`, forged by the delegate `username` (none for the genesis block).
pub fn block_row(
    block: &Block,
    username: Option<&str>,
    milestones: &Milestones,
) -> Result<BlockRow, StoreError> {
    let _ = (block, username, milestones);
    todo!("the block's row")
}

/// The rows of `block`'s transactions, in block order.
pub fn transaction_rows(
    block: &Block,
    milestones: &Milestones,
) -> Result<Vec<TransactionRow>, StoreError> {
    let _ = (block, milestones);
    todo!("the transactions' rows")
}

/// The block that `stored` holds, rebuilt from its row and its transactions' bytes, which this
/// node stored itself, so they are decoded without the transaction rules.
pub fn stored_block(stored: &StoredBlock, milestones: &Milestones) -> Result<Block, StoreError> {
    let _ = (stored, milestones);
    todo!("rebuild a stored block")
}

/// The rows of round `round`, whose delegates are `delegates` in rank order.
pub fn round_rows(round: u64, delegates: &[RoundDelegate]) -> Vec<RoundRow> {
    let _ = (round, delegates);
    todo!("the round's rows")
}

/// The rows of the slots `missed` before a block.
pub fn missed_rows(missed: &[MissedBlock]) -> Vec<MissedRow> {
    let _ = missed;
    todo!("the missed rows")
}
