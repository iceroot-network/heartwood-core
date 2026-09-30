//! `difftest store summary DIR`: a relay's store summarised as the rollback's live step records it,
//! read through a read-only connection, so that it also reads the store of a running relay.
//!
//! `DIR` is the relay's data directory (`CORE_PATH_DATA`), whose store is
//! `database/blockchain.db`. The summary is the oracle's (`chain.rollback`'s and `chain.restart`'s
//! `summary`) and a PostgreSQL summary's shape: `{tip, round, missed, transactions}`, the number of
//! blocks (the tip's height, in a store that holds every height from 1 to its tip), the highest
//! round with rows (0 when there is none), the number of missed-slot rows and the number of
//! transactions, the genesis block's included.

use std::path::Path;

use heartwood_database::{Readers, RowCounts, Settings};
use serde_json::{Value, json};

use crate::errors::{Error, Result};

/// The summary of the store in the data directory `dir`, as JSON.
pub fn summary(dir: &Path) -> Result<Value> {
    let settings = Settings::in_data_dir(dir);
    if !settings.path.is_file() {
        return Err(Error::Usage(format!(
            "{} holds no store ({} is not a file)",
            dir.display(),
            settings.path.display()
        )));
    }
    let store = |e: heartwood_database::DatabaseError| {
        Error::Usage(format!("{}: {e}", settings.path.display()))
    };
    let readers = Readers::open(&settings, 1).map_err(store)?;
    let counts = readers.get().row_counts(0, 0).map_err(store)?;
    Ok(value(&counts))
}

/// The summary of a store whose rows above 0 are `counts`.
pub fn value(counts: &RowCounts) -> Value {
    json!({
        "tip": counts.blocks,
        "round": counts.last_round,
        "missed": counts.missed,
        "transactions": counts.transactions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ScratchDir;

    #[test]
    fn a_directory_without_a_store_is_refused() {
        let dir = ScratchDir::new("store-summary").unwrap();
        let error = summary(dir.path()).unwrap_err().to_string();
        assert!(error.contains("holds no store"), "{error}");
    }

    #[test]
    fn the_counts_give_the_oracles_shape() {
        let counts = RowCounts {
            blocks: 212,
            transactions: 149,
            missed: 8,
            round_rows: 0,
            last_round: 5,
        };
        assert_eq!(
            value(&counts),
            json!({"tip": 212, "round": 5, "missed": 8, "transactions": 149})
        );
    }
}
