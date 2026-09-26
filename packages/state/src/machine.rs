//! The state machine: whole blocks applied and reverted in the reference's order.
//!
//! Applying block `H`: the missed slots since the last block are detected against the current
//! forging list; the forger must be a registered delegate; the transactions and the payout are
//! applied ([`crate::block_state`]); the block becomes the last block; the round state records it
//! and starts the next round when `H` ends one. Reverting the last block runs the same steps
//! backwards: the round first, then the payout, then the transactions in reverse order.
//!
//! Every applied block keeps its undo record in memory, so any number of blocks can be reverted.
//! A refused block leaves the state exactly as it was.

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::{Block, BlockHeader, Milestones};

use crate::errors::{ApplyError, GenesisError, RevertError, RewardError};
use crate::handlers::TransactionHandlers;
use crate::round_state::{MissedBlock, RoundDelegate, RoundReward, RoundState};
use crate::stores::state::{StateStore, Totals};
use crate::undo::BlockUndo;
use crate::wallets::WalletRepository;

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

impl State {
    /// The state after `genesis`, as the reference builds it at its first start (see
    /// [`crate::genesis`]).
    pub fn from_genesis<H: TransactionHandlers>(
        genesis: &Block,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<State, GenesisError> {
        crate::genesis::build(genesis, milestones, handlers)
    }

    /// Apply `block`, the block after the last one.
    pub fn apply_block<H: TransactionHandlers>(
        &mut self,
        block: &Block,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<Applied, ApplyError> {
        let _ = (block, milestones, handlers);
        todo!()
    }

    /// Revert the last block.
    pub fn revert_block<H: TransactionHandlers>(
        &mut self,
        milestones: &Milestones,
        handlers: &H,
    ) -> Result<Reverted, RevertError> {
        let _ = (milestones, handlers);
        todo!()
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
        let _ = (height, generator, milestones);
        todo!()
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
