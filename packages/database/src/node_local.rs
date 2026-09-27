//! Node-local data: the version each delegate's node last announced (`delegate_versions`), which
//! the pool reads to know which delegates operate a node. It never enters consensus and is kept
//! across restarts.

use rusqlite::TransactionBehavior;

use crate::connection::Database;
use crate::errors::DatabaseError;
use crate::values::{Columns, int};

/// A row of `delegate_versions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateVersionRow {
    /// The delegate's public key, lowercase hex.
    pub public_key: String,
    /// The round in which the version was announced.
    pub round: u64,
    /// The announced version.
    pub version: String,
}

impl Database {
    /// Every delegate version row, by public key.
    pub fn delegate_versions(&self) -> Result<Vec<DelegateVersionRow>, DatabaseError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT public_key, round, version FROM delegate_versions ORDER BY public_key",
        )?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("delegate_versions", row);
            out.push(DelegateVersionRow {
                public_key: columns.get(0)?,
                round: columns.uint(1)?,
                version: columns.get(2)?,
            });
        }
        Ok(out)
    }

    /// Replace every delegate version row with `rows`, in a transaction of its own (outside any
    /// chunk).
    pub fn put_delegate_versions(
        &mut self,
        rows: &[DelegateVersionRow],
    ) -> Result<(), DatabaseError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute("DELETE FROM delegate_versions", [])?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO delegate_versions (public_key, round, version) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (public_key) DO UPDATE SET round = excluded.round, \
                 version = excluded.version",
            )?;
            for row in rows {
                statement.execute(rusqlite::params![
                    row.public_key,
                    int("delegate_versions.round", row.round)?,
                    row.version,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}
