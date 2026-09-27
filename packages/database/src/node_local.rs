//! Node-local data: the version each delegate's node last announced (`delegate_versions`), which
//! the pool reads to know which delegates operate a node. It never enters consensus and is kept
//! across restarts.

use crate::connection::Database;
use crate::errors::DatabaseError;

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
        todo!("read the delegate versions")
    }

    /// Replace every delegate version row with `rows`, in a transaction of its own (outside any
    /// chunk).
    pub fn put_delegate_versions(
        &mut self,
        rows: &[DelegateVersionRow],
    ) -> Result<(), DatabaseError> {
        let _ = rows;
        todo!("replace the delegate versions")
    }
}
