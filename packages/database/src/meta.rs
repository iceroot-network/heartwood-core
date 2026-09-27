//! The chain's identity (`meta`) and the digests of the applied milestone entries
//! (`milestone_entries`).

use std::collections::BTreeMap;

use heartwood_crypto::BlockId;
use heartwood_crypto::utils::hex;

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;
use crate::values::Columns;

/// The keys of `meta`, in the order of [`ChainMeta`]'s fields.
const KEYS: [&str; 4] = ["schema_version", "nethash", "genesis_id", "state_format"];

/// What `meta` holds: which chain the store belongs to, and the formats of its tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainMeta {
    /// The schema version of the file.
    pub schema_version: u32,
    /// The network's nethash, lowercase hex.
    pub nethash: String,
    /// The id of the genesis block.
    pub genesis_id: BlockId,
    /// The format of the state records.
    pub state_format: u32,
}

/// A row of `milestone_entries`: the digest of the milestone entry at `height`, recorded when the
/// tip reached that height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MilestoneEntryRow {
    /// The entry's height.
    pub height: u32,
    /// The entry's digest.
    pub digest: [u8; 32],
}

impl Database {
    /// The chain's identity, once the first start has written it.
    pub fn meta(&self) -> Result<Option<ChainMeta>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT key, value FROM meta")?;
        let mut rows = statement.query([])?;
        let mut values = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("meta", row);
            values.insert(columns.get::<String>(0)?, columns.get::<String>(1)?);
        }
        if values.is_empty() {
            return Ok(None);
        }
        let mut take = |key: &str| {
            values
                .remove(key)
                .ok_or_else(|| corrupt(format!("the key {key} is missing")))
        };
        let number = |key: &str, text: String| {
            text.parse::<u32>()
                .map_err(|_| corrupt(format!("{key}: {text} is not a version")))
        };
        let meta = ChainMeta {
            schema_version: number(KEYS[0], take(KEYS[0])?)?,
            nethash: take(KEYS[1])?,
            genesis_id: BlockId::from_hex(&take(KEYS[2])?)
                .map_err(|error| corrupt(format!("{}: {error}", KEYS[2])))?,
            state_format: number(KEYS[3], take(KEYS[3])?)?,
        };
        match values.keys().next() {
            Some(key) => Err(corrupt(format!("unknown key {key}"))),
            None => Ok(Some(meta)),
        }
    }

    /// Every milestone entry row, by height.
    pub fn milestone_entries(&self) -> Result<Vec<MilestoneEntryRow>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT height, digest FROM milestone_entries ORDER BY height")?;
        let mut rows = statement.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let columns = Columns::new("milestone_entries", row);
            out.push(MilestoneEntryRow {
                height: columns.uint(0)?,
                digest: columns.digest(1)?,
            });
        }
        Ok(out)
    }
}

/// A `meta` table that cannot be read.
fn corrupt(detail: String) -> DatabaseError {
    DatabaseError::CorruptRow {
        table: "meta",
        detail,
    }
}

impl Chunk<'_> {
    /// Write the chain's identity.
    pub fn put_meta(&mut self, meta: &ChainMeta) -> Result<(), DatabaseError> {
        let values = [
            meta.schema_version.to_string(),
            meta.nethash.clone(),
            meta.genesis_id.to_hex(),
            meta.state_format.to_string(),
        ];
        let mut statement = self.transaction.prepare_cached(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in KEYS.iter().zip(values) {
            statement.execute([*key, value.as_str()])?;
        }
        Ok(())
    }

    /// Record the digest of the milestone entry at `row.height`.
    pub fn put_milestone_entry(&mut self, row: &MilestoneEntryRow) -> Result<(), DatabaseError> {
        self.transaction
            .prepare_cached(
                "INSERT INTO milestone_entries (height, digest) VALUES (?1, ?2) \
                 ON CONFLICT (height) DO UPDATE SET digest = excluded.digest",
            )?
            .execute(rusqlite::params![row.height, hex::encode(&row.digest)])?;
        Ok(())
    }
}
