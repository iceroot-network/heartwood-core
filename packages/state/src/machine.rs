//! The state machine: whole blocks applied and reverted in the reference's order.
//!
//! Applying block `H`: the missed slots since the last block are detected against the current
//! forging list; the forger must be a registered delegate; the transactions and the payout are
//! applied ([`crate::block_state`]); the block becomes the last block; the round state records it
//! and starts the next round when `H` ends one. Reverting the last block runs the same steps
//! backwards: the round first, then the payout, then the transactions in reverse order.
//!
//! Every applied block keeps its undo record in memory, so any number of blocks can be reverted.
//! A refused block leaves the state exactly as it was. The one exception is a failure to take back
//! the steps already taken, which no consistent state reaches
//! ([`RollbackFailed`](crate::errors::RollbackFailed)).

use std::collections::BTreeMap;

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::{Block, BlockHeader, Milestones};
use heartwood_kernel::calculate_round;

use crate::block_state;
use crate::dpos::dpos::set_rank;
use crate::errors::{ApplyError, GenesisError, RevertError, RewardError, rolled_back};
use crate::handlers::TransactionHandlers;
use crate::persist::{RestoreError, STATE_FORMAT, StateRecord, TotalsRecord, bad};
use crate::round_state::{MissedBlock, RoundDelegate, RoundReward, RoundState};
use crate::stores::state::{StateStore, Totals};
use crate::supply::check_supply;
use crate::undo::BlockUndo;
use crate::wallets::{Wallet, WalletRepository};

/// The consensus state after the last applied block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    wallets: WalletRepository,
    round: RoundState,
    store: StateStore,
    journal: Vec<BlockUndo>,
}

/// What applying a block found and changed besides the wallets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The slots missed before the block, in slot order.
    pub missed: Vec<MissedBlock>,
    /// The round the block started, when it ended one.
    pub round_started: Option<RoundStart>,
}

/// A round that started: the rows the reference saves for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundStart {
    /// The round number.
    pub round: u64,
    /// The round's delegates in rank order.
    pub delegates: Vec<RoundDelegate>,
}

/// What reverting the last block did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reverted {
    /// The reverted block.
    pub block: Block,
    /// The round the block had started, which the revert took back.
    pub round_reverted: Option<u64>,
}

/// The round state and ranks before a revert, kept until the revert succeeds.
struct SavedRound {
    round: RoundState,
    ranks: Option<Vec<(Address, Option<u32>)>>,
}

impl State {
    /// The state after `genesis`, as the reference builds it at its first start (see
    /// [`crate::genesis`]).
    pub fn from_genesis<H: TransactionHandlers>(
        genesis: &Block,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<State, GenesisError<H::Error>> {
        crate::genesis::build(genesis, milestones, handlers)
    }

    /// The state made of these parts, as the genesis block builds it.
    pub(crate) fn from_parts(
        wallets: WalletRepository,
        round: RoundState,
        store: StateStore,
    ) -> State {
        State {
            wallets,
            round,
            store,
            journal: Vec::new(),
        }
    }

    /// Apply `block`, the block after the last one: detect the slots missed before it, apply its
    /// transactions and pay its forger, make it the last block, and record it in the round,
    /// starting the next round when it ends one. On an error the state is unchanged (see the
    /// module documentation for the one exception).
    ///
    /// The block's rules (chaining, signatures, the scheduled generator, the reward, and a
    /// timestamp that is not in the future) are the block processor's, which refuses a block that
    /// breaks one before it calls this; this only requires the height to follow and the generator
    /// to be a registered delegate, and checks both before it detects the missed slots. Missed
    /// slots are detected one row per slot between the last block and `block`, so their number
    /// is bounded only by the processor's timestamp rule.
    pub fn apply_block<H: TransactionHandlers>(
        &mut self,
        block: &Block,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<Applied, ApplyError<H::Error>> {
        let height = block.height();
        if self.height().checked_add(1) != Some(height) {
            return Err(ApplyError::Height {
                expected: self.height().saturating_add(1),
                height,
            });
        }
        let generator = block.header().generator_public_key;
        let username = self
            .wallets
            .find_by_public_key(&generator)
            .and_then(Wallet::delegate)
            .map(|delegate| delegate.username().to_owned())
            .ok_or(ApplyError::GeneratorNotDelegate { generator })?;
        let missed = self.detect_missed_blocks(block.header(), milestones);

        let mut undo = block_state::apply_block(
            &mut self.wallets,
            self.store.totals_mut(),
            block,
            milestones,
            handlers,
        )?;
        let previous_block = self.store.replace_last_block(block.clone());
        // On an error the round state keeps its round and every rank as they were.
        match self
            .round
            .apply_block(&mut self.wallets, height, &username, milestones)
        {
            Ok(round) => undo.round = round,
            Err(error) => {
                let undone = block_state::revert_block(
                    &mut self.wallets,
                    self.store.totals_mut(),
                    &undo,
                    milestones,
                    handlers,
                );
                self.store.set_last_block(previous_block);
                rolled_back("block", undone)?;
                return Err(error.into());
            }
        }
        let round_started = undo.round.as_ref().map(|_| RoundStart {
            round: self.round.info().round,
            delegates: self.round.round_delegates().to_vec(),
        });
        self.journal.push(undo);
        Ok(Applied {
            missed,
            round_started,
        })
    }

    /// Revert the last block: take back the round it started, if any, then its forger's payout,
    /// then its transactions in reverse order; the block before it becomes the last block. On an
    /// error the state is unchanged.
    pub fn revert_block<H: TransactionHandlers>(
        &mut self,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<Reverted, RevertError<H::Error>> {
        let undo = self.journal.pop().ok_or(RevertError::Genesis)?;
        let height = undo.block.height();
        let saved = self.save_round(undo.round.is_some());
        let round_reverted = undo.round.as_ref().map(|_| self.round.info().round);
        let result = self
            .round
            .revert_block(&mut self.wallets, height, undo.round.clone())
            .map_err(RevertError::from)
            .and_then(|()| {
                if undo.round.is_some() {
                    defer_round_reindex(&mut self.wallets, &self.journal, &undo, milestones);
                }
                block_state::revert_block(
                    &mut self.wallets,
                    self.store.totals_mut(),
                    &undo,
                    milestones,
                    handlers,
                )
            });
        if let Err(error) = result {
            self.restore_round(saved);
            self.journal.push(undo);
            return Err(error);
        }
        let previous = self
            .journal
            .last()
            .map_or_else(|| self.store.genesis_block(), |previous| &previous.block)
            .clone();
        self.store.set_last_block(previous);
        Ok(Reverted {
            block: undo.block,
            round_reverted,
        })
    }

    /// The round state and, when `with_ranks`, every delegate's rank, to restore them if a revert
    /// fails after the round's part.
    fn save_round(&self, with_ranks: bool) -> SavedRound {
        SavedRound {
            round: self.round.clone(),
            ranks: with_ranks.then(|| {
                self.wallets
                    .delegates()
                    .filter_map(|wallet| Some((*wallet.address(), wallet.delegate()?.rank)))
                    .collect()
            }),
        }
    }

    /// Put back what [`State::save_round`] saved.
    fn restore_round(&mut self, saved: SavedRound) {
        self.round = saved.round;
        for (address, rank) in saved.ranks.into_iter().flatten() {
            set_rank(&mut self.wallets, &address, rank);
        }
    }

    /// The slots missed between the last block and `next`, without applying anything: one row
    /// per slot between them, so the caller first refuses a `next` whose timestamp lies in the
    /// future, as the reference's block processor does.
    pub fn detect_missed_blocks(
        &self,
        next: &BlockHeader,
        milestones: &Milestones,
    ) -> Vec<MissedBlock> {
        self.round
            .detect_missed_blocks(self.store.last_block().header(), next, milestones)
    }

    /// The reward of a block at `height` forged by the delegate with key `generator`.
    pub fn reward_for_block_in_round(
        &self,
        height: u32,
        generator: &PublicKeyBytes,
        milestones: &Milestones,
    ) -> Result<RoundReward, RewardError> {
        let delegate = self
            .wallets
            .find_by_public_key(generator)
            .and_then(Wallet::delegate)
            .ok_or(RewardError::NotADelegate {
                generator: *generator,
            })?;
        self.round
            .reward_for_block_in_round(height, delegate, milestones)
    }

    /// The forging list that schedules the next block.
    pub fn forging_delegates(&self) -> &[RoundDelegate] {
        self.round.forging_delegates()
    }

    /// The height of the last block.
    pub fn height(&self) -> u32 {
        self.store.last_block().height()
    }

    /// The last applied block.
    pub fn last_block(&self) -> &Block {
        self.store.last_block()
    }

    /// The genesis block.
    pub fn genesis_block(&self) -> &Block {
        self.store.genesis_block()
    }

    /// The wallets.
    pub fn wallets(&self) -> &WalletRepository {
        &self.wallets
    }

    /// Re-index the wallets at `addresses` that are in the address index, in order (see
    /// [`WalletRepository::reindex_parties`]): the parties of the transactions a pool or a
    /// collation applied or reverted on its copy of the wallets. Only the indexes' orders change.
    pub fn reindex_parties(&mut self, addresses: impl IntoIterator<Item = Address>) {
        self.wallets.reindex_parties(addresses);
    }

    /// The round state.
    pub fn round(&self) -> &RoundState {
        &self.round
    }

    /// The supply totals.
    pub fn totals(&self) -> &Totals {
        self.store.totals()
    }

    /// The undo records of the applied blocks, oldest first.
    pub fn journal(&self) -> &[BlockUndo] {
        &self.journal
    }
}

/// The state read back from the store (see [`crate::persist`]).
impl State {
    /// The record of the tip's state besides the wallets.
    pub fn state_record(&self) -> StateRecord {
        StateRecord {
            format: STATE_FORMAT,
            height: self.height(),
            block_id: self.last_block().id().to_hex(),
            totals: TotalsRecord::from_totals(self.totals()),
            round: self.round.to_record(),
        }
    }

    /// The state made of what the store holds: the genesis block, the last block, the wallets
    /// read back from their records, the state record, and the undo record of every block from
    /// height 2 to the tip, in order. Refused when the parts do not make a consistent state (see
    /// [`crate::persist`]); the state is then to be replayed from the genesis block.
    pub fn restore(
        genesis: Block,
        last: Block,
        wallets: WalletRepository,
        record: StateRecord,
        journal: Vec<BlockUndo>,
        milestones: &Milestones,
    ) -> Result<State, RestoreError> {
        if record.format != STATE_FORMAT {
            return Err(RestoreError::Format(record.format));
        }
        let height = last.height();
        let tip = journal.last().map_or(&genesis, |undo| &undo.block);
        if (record.height, &record.block_id) != (height, &last.id().to_hex()) || tip != &last {
            return Err(RestoreError::LastBlock {
                record_height: record.height,
                record_id: record.block_id,
                block_height: height,
                block_id: last.id().to_hex(),
            });
        }
        // One record per height from 2, in order; the last one is the last block's (checked above).
        let mut heights = journal.iter().map(|undo| undo.block.height());
        if let Some(expected) = (2..=height).find(|expected| heights.next() != Some(*expected)) {
            return Err(RestoreError::Journal { expected });
        }
        let totals = record.totals.to_totals()?;
        let round = RoundState::from_record(record.round)?;
        let (info, next) = (
            round.info(),
            calculate_round(height.saturating_add(1), milestones),
        );
        if (info.round, info.round_height) != (next.round, next.round_height) {
            return Err(bad("round", format!("not the round after height {height}")));
        }
        check_vote_balances(&wallets)?;
        let mut store = StateStore::new(genesis, totals.issued);
        store.set_last_block(last);
        *store.totals_mut() = totals;
        let mut state = State::from_parts(wallets, round, store);
        state.journal = journal;
        check_supply(&state)?;
        Ok(state)
    }
}

/// Every delegate's vote balance and voter count must be the sum and the number of its voters'
/// shares, as the reference's start-up builds them, and every share must name a delegate.
fn check_vote_balances(wallets: &WalletRepository) -> Result<(), RestoreError> {
    let mut sums: BTreeMap<&str, (u128, u64)> = BTreeMap::new();
    for (name, share) in wallets.iter().flat_map(Wallet::vote_shares) {
        let (balance, voters) = sums.entry(name).or_default();
        *balance = balance.saturating_add((*share).into());
        *voters = voters.saturating_add(1);
    }
    let wrong = |username: &str| RestoreError::VoteBalance {
        username: username.to_owned(),
    };
    for delegate in wallets.delegates().filter_map(Wallet::delegate) {
        let sum = sums.remove(delegate.username()).unwrap_or_default();
        if sum != (delegate.vote_balance.into(), delegate.voters) {
            return Err(wrong(delegate.username()));
        }
    }
    let unknown = sums.into_keys().next();
    unknown.map_or(Ok(()), |name| Err(wrong(name)))
}

/// Queue for a re-index the senders and recipients of the round that `undo`'s block ended, from
/// that block back to the round's first block, each block's transactions in reverse order: the
/// reference reverts them on a copy of the wallets to rank the round again, and its handlers
/// re-index the state's wallets as they go (see [`crate::wallets::indexes`]).
fn defer_round_reindex(
    wallets: &mut WalletRepository,
    journal: &[BlockUndo],
    undo: &BlockUndo,
    milestones: &Milestones,
) {
    let first = calculate_round(undo.block.height(), milestones).round_height;
    let earlier = journal.iter().rev();
    let round =
        std::iter::once(undo).chain(earlier.take_while(|e| u64::from(e.block.height()) >= first));
    for block in round.map(|undo| &undo.block) {
        for transaction in block.transactions().iter().rev() {
            wallets.defer_reindex(transaction);
        }
    }
}
