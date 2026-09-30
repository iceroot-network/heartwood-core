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
use heartwood_database::{ChainMeta, Chunk, Database, MilestoneEntryRow, Settings, StateRow};
use heartwood_kernel::{calculate_round, is_new_round};
use heartwood_state::machine::{Applied, Reverted};
use heartwood_state::persist::{STATE_FORMAT, UndoRecord};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::{Delegate, State, Wallet, WalletRepository};

use crate::errors::StoreError;
use crate::milestones::reached;
use crate::rows::{block_row, missed_rows, round_rows, transaction_rows};

/// A node's store, with the committed view of the wallets and the digests of the configured
/// milestone entries.
#[derive(Debug)]
pub struct ChainStore {
    database: Database,
    /// The wallets as the store holds them, set by [`crate::restore()`] and by every commit.
    pub(crate) committed: Option<WalletRepository>,
    /// The digests of the configured milestone entries, set by [`crate::restore()`].
    pub(crate) entries: Vec<MilestoneEntryRow>,
    /// The chain's identity, set by [`crate::restore()`].
    pub(crate) meta: Option<ChainMeta>,
}

impl ChainStore {
    /// Open the database of `settings`. The committed view and the milestone entries are set by
    /// [`crate::restore()`].
    pub fn open(settings: &Settings) -> Result<ChainStore, StoreError> {
        Ok(ChainStore {
            database: Database::open(settings)?,
            committed: None,
            entries: Vec::new(),
            meta: None,
        })
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

    /// Start a chunk. Refused until [`crate::restore()`] has started the store: before that the
    /// store knows neither the chain's identity nor the milestone entries' digests, and a chunk
    /// written without them would refuse the next start.
    pub fn chunk(&mut self) -> Result<ChunkWriter<'_>, StoreError> {
        let ChainStore {
            database,
            committed,
            entries,
            meta,
        } = self;
        if meta.is_none() {
            return Err(StoreError::NotStarted);
        }
        Ok(ChunkWriter {
            chunk: database.begin()?,
            committed,
            entries,
            meta: meta.as_ref(),
            wallets: None,
        })
    }
}

/// The writes of one chunk. Dropping it without [`ChunkWriter::commit`] writes nothing.
#[derive(Debug)]
pub struct ChunkWriter<'s> {
    chunk: Chunk<'s>,
    committed: &'s mut Option<WalletRepository>,
    entries: &'s [MilestoneEntryRow],
    meta: Option<&'s ChainMeta>,
    /// The wallets after the chunk's last block, which the next block's changes are found from.
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
        let genesis = state.genesis_block();
        self.chunk.put_meta(meta)?;
        let row = block_row(genesis, None, milestones)?;
        self.chunk
            .save_block(&row, &transaction_rows(genesis, milestones)?)?;
        let round = state.round();
        let rows = round_rows(round.info().round, round.round_delegates());
        self.chunk.save_round(&rows)?;
        self.write_state(state, 0, true)
    }

    /// Write what applying `block` changed: `state` is the state after it, `applied` what the
    /// apply returned. The block is the last one of `state`, whose undo record is written with
    /// it.
    pub fn applied(
        &mut self,
        state: &State,
        block: &Block,
        applied: &Applied,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let height = block.height();
        let undo = state
            .journal()
            .last()
            .filter(|undo| undo.block.id() == block.id());
        let undo = undo.ok_or_else(|| encode("undo", "the block is not the state's last block"))?;
        let generator = state
            .wallets()
            .find_by_public_key(&block.header().generator_public_key);
        let username = generator.and_then(Wallet::delegate).map(Delegate::username);
        let row = block_row(block, username, milestones)?;
        self.chunk
            .save_block(&row, &transaction_rows(block, milestones)?)?;
        if let Some(start) = &applied.round_started {
            self.chunk
                .save_round(&round_rows(start.round, &start.delegates))?;
        }
        self.chunk.save_missed(&missed_rows(&applied.missed))?;
        let record = serde_json::to_string(&UndoRecord::from_undo(undo));
        self.chunk
            .put_undo(height, &record.map_err(|e| encode("undo", e))?)?;
        self.write_state(state, height.saturating_sub(1), false)
    }

    /// Write what reverting the last block changed: `state` is the state after the revert. The
    /// rows above the new tip go, and the round rows above the round of the new tip's block, as
    /// the reference deletes them.
    pub fn reverted(
        &mut self,
        state: &State,
        reverted: &Reverted,
        milestones: &Milestones,
    ) -> Result<(), StoreError> {
        let height = state.height();
        if reverted.block.height() != height.saturating_add(1) {
            return Err(encode(
                "revert",
                "the reverted block is not above the state's tip",
            ));
        }
        let round = calculate_round(height, milestones).round;
        self.chunk.delete_above(height, round)?;
        self.write_state(state, height, false)
    }

    /// Write every state table again from `state`: every wallet, the state row and every undo
    /// record, as after a replay from the genesis block; and the state format in the chain's
    /// identity.
    pub fn rewrite_state(&mut self, state: &State) -> Result<(), StoreError> {
        self.chunk.delete_wallets()?;
        for undo in state.journal() {
            let record = serde_json::to_string(&UndoRecord::from_undo(undo));
            let record = record.map_err(|e| encode("undo", e))?;
            self.chunk.put_undo(undo.block.height(), &record)?;
        }
        if let Some(meta) = self.meta {
            let meta = ChainMeta {
                state_format: STATE_FORMAT,
                ..meta.clone()
            };
            self.chunk.put_meta(&meta)?;
        }
        self.write_state(state, state.height(), true)
    }

    /// Save the missed-slot rows that a start counts (see [`crate::restore()`]).
    pub(crate) fn save_missed(&mut self, missed: &[MissedBlock]) -> Result<(), StoreError> {
        Ok(self.chunk.save_missed(&missed_rows(missed))?)
    }

    /// At a start whose tip ends a round (`is_new_round(tip + 1)`), or is the genesis block, as the
    /// reference's condition `height == 1 || isNewRound(height + 1)` reads, write the rows of round
    /// N, the round of the state's forging list, from `state`, as the reference's start does
    /// (`RoundState.restore`: its `deleteRound` of the rounds above the tip's, then
    /// `applyRound(tip)`, which saves round N's rows): the chunk's `delete_above(tip, the tip's
    /// round)`, which at the tip removes the round rows above the tip's round and nothing else,
    /// then `save_round` of round N's rows. Whether they were written. After an ordinary stop at a
    /// round end the rows are there already and equal; after a rollback, or a revert to a round
    /// end followed by a stop, they are not, and this writes them again.
    pub fn next_round_at_start(
        &mut self,
        state: &State,
        milestones: &Milestones,
    ) -> Result<bool, StoreError> {
        let tip = state.height();
        if tip != 1 && !is_new_round(tip.saturating_add(1), milestones) {
            return Ok(false);
        }
        let round = state.round();
        let tip_round = calculate_round(tip, milestones).round;
        self.chunk.delete_above(tip, tip_round)?;
        self.chunk
            .save_round(&round_rows(round.info().round, round.round_delegates()))?;
        Ok(true)
    }

    /// Write the wallets that changed since the chunk's last block (every wallet when `all`), a
    /// wallet back to its empty value by deleting its row; the state row; and the milestone
    /// entries that the tip reaches from `from`.
    fn write_state(&mut self, state: &State, from: u32, all: bool) -> Result<(), StoreError> {
        let base = self.wallets.as_ref().or(self.committed.as_ref());
        let wallets: Vec<&Wallet> = match base.filter(|_| !all) {
            Some(base) => state.wallets().changed_since(base).collect(),
            None => state.wallets().iter().collect(),
        };
        for wallet in wallets {
            let address = wallet.address().to_base58();
            if *wallet == Wallet::new(*wallet.address()) {
                // A wallet back to its empty value; with `all`, the table has no row for it.
                if !all {
                    self.chunk.delete_wallet(&address)?;
                }
            } else {
                let record = serde_json::to_string(&wallet.to_record());
                self.chunk
                    .put_wallet(&address, &record.map_err(|e| encode("wallet", e))?)?;
            }
        }
        let record = serde_json::to_string(&state.state_record());
        self.chunk.put_state(&StateRow {
            height: state.height(),
            block_id: *state.last_block().id(),
            record: record.map_err(|e| encode("state", e))?,
        })?;
        for entry in reached(self.entries, from, state.height()) {
            self.chunk.put_milestone_entry(&entry)?;
        }
        self.wallets = Some(state.wallets().clone());
        Ok(())
    }

    /// Commit the chunk; its last wallets become the committed view.
    pub fn commit(self) -> Result<(), StoreError> {
        self.chunk.commit()?;
        if let Some(wallets) = self.wallets {
            *self.committed = Some(wallets);
        }
        Ok(())
    }
}

/// The error of a record that cannot be written.
fn encode(what: &'static str, detail: impl std::fmt::Display) -> StoreError {
    let detail = detail.to_string();
    StoreError::Encode { what, detail }
}
