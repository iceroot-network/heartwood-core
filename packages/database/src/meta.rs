//! The chain's identity (`meta`) and the digests of the applied milestone entries
//! (`milestone_entries`).

use heartwood_crypto::BlockId;

use crate::connection::{Chunk, Database};
use crate::errors::DatabaseError;

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
        todo!("read meta")
    }

    /// Every milestone entry row, by height.
    pub fn milestone_entries(&self) -> Result<Vec<MilestoneEntryRow>, DatabaseError> {
        todo!("read the milestone entries")
    }
}

impl Chunk<'_> {
    /// Write the chain's identity.
    pub fn put_meta(&mut self, meta: &ChainMeta) -> Result<(), DatabaseError> {
        let _ = meta;
        todo!("write meta")
    }

    /// Record the digest of the milestone entry at `row.height`.
    pub fn put_milestone_entry(&mut self, row: &MilestoneEntryRow) -> Result<(), DatabaseError> {
        let _ = row;
        todo!("write a milestone entry")
    }
}
