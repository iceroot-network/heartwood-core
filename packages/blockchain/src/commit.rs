//! The commit: what an applied or reverted block changes, written to the store in the chunk's one
//! transaction.
//!
//! For an applied block: its row and its transactions' rows, the round's rows when it starts a
//! round, the missed rows found before it, its undo record, every wallet that changed since the
//! last committed view (a wallet back to its empty value is deleted), the state row, and the
//! digest of each milestone entry whose height the new tip reaches. For a reverted block: the
//! rows above the new tip (see [`heartwood_database::Chunk::delete_above`]), then the changed
//! wallets and the state row. When the chunk commits, the chunk's last wallets become the
//! committed view that the next chunk compares with.

use heartwood_crypto::{Block, Milestones};
use heartwood_database::{ChainMeta, Chunk, Database, MilestoneEntryRow, Settings};
use heartwood_state::machine::{Applied, Reverted};
use heartwood_state::{State, WalletRepository};

use crate::errors::StoreError;

/// A node's store, with the committed view of the wallets and the digests of the configured
/// milestone entries.
#[derive(Debug)]
pub struct ChainStore {
    database: Database,
    committed: Option<WalletRepository>,
    entries: Vec<MilestoneEntryRow>,
}

impl ChainStore {
    /// Open the database of `settings`. The committed view and the milestone entries are set by
    /// [`crate::restore()`].
    pub fn open(settings: &Settings) -> Result<ChainStore, StoreError> {
        let _ = settings;
        todo!("open the database")
    }

    /// The database, to read it.
    pub fn database(&self) -> &Database {
        &self.database
    }

    /// The database, for the node-local writes outside a chunk.
    pub fn database_mut(&mut self) -> &mut Database {
        &mut self.database
    }

    /// The digests of the configured milestone entries, by height.
    pub fn milestone_entries(&self) -> &[MilestoneEntryRow] {
        &self.entries
    }

    /// Start a chunk.
    pub fn chunk(&mut self) -> Result<ChunkWriter<'_>, StoreError> {
        let ChainStore {
            database,
            committed,
            entries,
        } = self;
        Ok(ChunkWriter {
            chunk: database.begin()?,
            committed,
            entries,
            wallets: None,
        })
    }
}

/// The writes of one chunk. Dropping it without [`ChunkWriter::commit`] writes nothing.
#[derive(Debug)]
#[expect(dead_code, reason = "the commit's methods read the fields")]
pub struct ChunkWriter<'s> {
    chunk: Chunk<'s>,
    committed: &'s mut Option<WalletRepository>,
    entries: &'s [MilestoneEntryRow],
    wallets: Option<WalletRepository>,
}

impl ChunkWriter<'_> {
    /// Write the first start: `meta`, the genesis block of `state` with its transactions, round
    /// 1's rows, every wallet, the state row, and the milestone entry at height 1.
    pub fn first_start(
        &mut self,
        state: &State,
        meta: &ChainMeta,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let _ = (state, meta, milestones);
        todo!("write the first start")
    }

    /// Write what applying `block` changed: `state` is the state after it, `applied` what the
    /// state machine returned.
    pub fn applied(
        &mut self,
        state: &State,
        block: &Block,
        applied: &Applied,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let _ = (state, block, applied, milestones);
        todo!("write an applied block")
    }

    /// Write what reverting the last block changed: `state` is the state after the revert.
    pub fn reverted(
        &mut self,
        state: &State,
        reverted: &Reverted,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let _ = (state, reverted, milestones);
        todo!("write a reverted block")
    }

    /// Write every state table again from `state`: every wallet, the state row and every undo
    /// record, as after a replay from the genesis block.
    pub fn rewrite_state(
        &mut self,
        state: &State,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let _ = (state, milestones);
        todo!("write the state tables again")
    }

    /// Commit the chunk; its last wallets become the committed view.
    pub fn commit(self) -> Result<(), StoreError> {
        todo!("commit the chunk")
    }
}
