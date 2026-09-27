//! Round rows: the delegates of each round with their vote balances at the round start.

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::values::{Columns, int};

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
        let mut statement = self.connection.prepare_cached(
            "SELECT round, public_key, balance FROM rounds WHERE round = ?1 \
             ORDER BY balance DESC, public_key COLLATE BINARY ASC",
        )?;
        let mut rows = statement.query([int("rounds.round", round)?])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("rounds", row);
            out.push(RoundRow {
                round: columns.uint(0)?,
                public_key: columns.get(1)?,
                balance: columns.uint(2)?,
            });
        }
        Ok(out)
    }
}

impl Chunk<'_> {
    /// Save the rows of a round that starts. A row of the same round and key is replaced, as the
    /// reference's `save` replaces an entity with the same primary key.
    pub fn save_round(&mut self, rows: &[RoundRow]) -> Result<(), DatabaseError> {
        let mut statement = self.transaction.prepare_cached(
            "INSERT INTO rounds (round, public_key, balance) VALUES (?1, ?2, ?3) \
             ON CONFLICT (round, public_key) DO UPDATE SET balance = excluded.balance",
        )?;
        for row in rows {
            statement.execute(rusqlite::params![
                int("rounds.round", row.round)?,
                row.public_key,
                int("rounds.balance", row.balance)?,
            ])?;
        }
        Ok(())
    }
}
