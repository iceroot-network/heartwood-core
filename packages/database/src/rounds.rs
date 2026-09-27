//! Round rows: the delegates of each round with their vote balances at the round start.

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;

/// A row of `rounds`: one delegate of a round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundRow {
    /// The round.
    pub round: u64,
    /// The delegate's public key, lowercase hex.
    pub public_key: String,
    /// The delegate's vote balance at the round start.
    pub balance: u64,
}

impl Database {
    /// The rows of `round`, by balance from the highest, then by public key in byte order: the
    /// order the reference's `getRound` gives in a database with the C collation.
    pub fn round(&self, round: u64) -> Result<Vec<RoundRow>, DatabaseError> {
        let _ = round;
        todo!("read a round")
    }
}

impl Chunk<'_> {
    /// Save the rows of a round that starts.
    pub fn save_round(&mut self, rows: &[RoundRow]) -> Result<(), DatabaseError> {
        let _ = rows;
        todo!("insert the round rows")
    }
}
