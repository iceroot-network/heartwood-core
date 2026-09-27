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

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::{Block, BlockHeader, Milestones};
use heartwood_kernel::is_new_round;

use crate::block_state;
use crate::errors::{ApplyError, GenesisError, RevertError, RewardError, rolled_back};
use crate::handlers::TransactionHandlers;
use crate::round_state::{MissedBlock, RoundDelegate, RoundReward, RoundState};
use crate::stores::state::{StateStore, Totals};
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

/// The round state and ranks before a round step, kept until the step succeeds.
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
    /// starting the next round when it ends one. On an error the state is unchanged.
    ///
    /// The block's rules (chaining, signatures, the scheduled generator, the reward) are the
    /// block processor's; this only requires the height to follow and the generator to be a
    /// registered delegate.
    pub fn apply_block<H: TransactionHandlers>(
        &mut self,
        block: &Block,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<Applied, ApplyError<H::Error>> {
        let height = block.height();
        let expected = self
            .height()
            .checked_add(1)
            .filter(|expected| *expected == height)
            .ok_or(ApplyError::Height {
                expected: self.height().saturating_add(1),
                height,
            })?;
        let missed = self.detect_missed_blocks(block.header(), milestones);
        let generator = block.header().generator_public_key;
        let username = self
            .wallets
            .find_by_public_key(&generator)
            .and_then(Wallet::delegate)
            .map(|delegate| delegate.username().to_owned())
            .ok_or(ApplyError::GeneratorNotDelegate { generator })?;

        let mut undo = block_state::apply_block(
            &mut self.wallets,
            self.store.totals_mut(),
            block,
            milestones,
            handlers,
        )?;
        let previous_block = self.store.replace_last_block(block.clone());
        let saved = self.save_round(is_new_round(expected.saturating_add(1), milestones));
        match self
            .round
            .apply_block(&mut self.wallets, height, &username, milestones)
        {
            Ok(round) => undo.round = round,
            Err(error) => {
                self.restore_round(saved);
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

    /// The round state and, when `with_ranks`, every delegate's rank, to restore them if a round
    /// step fails.
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
            if let Some(delegate) = self
                .wallets
                .wallet_mut(&address)
                .and_then(Wallet::delegate_mut)
            {
                delegate.rank = rank;
            }
        }
    }

    /// The slots missed between the last block and `next`, without applying anything.
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
