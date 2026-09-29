//! `difftest store summary DIR`: a relay's store summarised as the rollback's live step records it
//! (the tip, the highest round row, the numbers of missed-slot rows and of transactions), read
//! through a read-only connection, so that it also reads the store of a running relay.

use std::path::Path;

use serde_json::Value;

use crate::errors::{Error, Result};

/// The summary of the store in the data directory `dir`, as JSON.
pub fn summary(dir: &Path) -> Result<Value> {
    let _ = dir;
    Err(Error::NotYet(
        "store summary has no Rust implementation yet".into(),
    ))
}
