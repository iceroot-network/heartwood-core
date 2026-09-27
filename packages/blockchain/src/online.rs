//! The node-local online record in the store: the pool's [`DelegateVersions`] (the version each
//! delegate's node last announced, and the round in which it did) as the rows of the
//! `delegate_versions` table. The node saves the record when it changes, outside any chunk
//! ([`ChainStore::save_online`]), and every start loads it ([`crate::Restored::online`]), so it
//! survives a restart with each entry's round, and so its expiry. It never enters consensus.

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_database::DelegateVersionRow;
use heartwood_pool::{DelegateVersions, OnlineEntry};

use crate::commit::ChainStore;
use crate::errors::StoreError;

/// The rows of `record`, by public key.
pub fn rows(record: &DelegateVersions) -> Vec<DelegateVersionRow> {
    let row = |entry: OnlineEntry| DelegateVersionRow {
        public_key: entry.public_key.to_hex(),
        round: entry.round,
        version: entry.version,
    };
    record.entries().into_iter().map(row).collect()
}

/// The record that `rows` hold. A row whose key is not the hex of 33 bytes does not read back.
pub fn record(rows: Vec<DelegateVersionRow>) -> Result<DelegateVersions, StoreError> {
    let entry = |row: DelegateVersionRow| {
        let public_key = PublicKeyBytes::from_hex(&row.public_key).map_err(|e| {
            let detail = format!("the key {:?}: {e}", row.public_key);
            StoreError::Decode {
                what: "delegate version",
                detail,
            }
        })?;
        Ok(OnlineEntry {
            public_key,
            round: row.round,
            version: row.version,
        })
    };
    let entries = rows.into_iter().map(entry);
    Ok(DelegateVersions::from_entries(
        entries.collect::<Result<Vec<_>, StoreError>>()?,
    ))
}

impl ChainStore {
    /// Save `record` in place of the stored online record, in a transaction of its own.
    pub fn save_online(&mut self, record: &DelegateVersions) -> Result<(), StoreError> {
        self.database_mut().put_delegate_versions(&rows(record))?;
        Ok(())
    }

    /// The stored online record.
    pub fn online(&self) -> Result<DelegateVersions, StoreError> {
        record(self.database().delegate_versions()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_round_trip_and_a_bad_key_does_not_read_back() {
        let mut versions = DelegateVersions::new();
        versions.set(PublicKeyBytes::from_array([3; 33]), 7, "4.3.1".into());
        versions.set(PublicKeyBytes::from_array([2; 33]), 9, "4.3.2".into());
        let rows = rows(&versions);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].public_key, "02".repeat(33));
        assert_eq!((rows[0].round, rows[0].version.as_str()), (9, "4.3.2"));
        assert_eq!(record(rows).unwrap(), versions);
        let bad = DelegateVersionRow {
            public_key: "02".into(),
            round: 1,
            version: "v".into(),
        };
        assert!(matches!(
            record(vec![bad]),
            Err(StoreError::Decode {
                what: "delegate version",
                ..
            })
        ));
    }
}
