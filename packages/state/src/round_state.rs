//! The round state: the delegates of the current round, their shuffled forging list, and the
//! blocks forged in the round so far.
//!
//! A round starts after the block that ends the previous one (and after the genesis block): the
//! delegates are ranked, the first `activeDelegates` become the round's delegates, and their
//! forging list is their shuffle for the round. A rank is the snapshot taken at the round start;
//! reverting the block that started a round restores the previous round and every rank exactly.
//!
//! The shuffle is the unbiased Durstenfeld shuffle over a SHA-256 stream: the stream starts with
//! `SHA-256(decimal round)` and continues with the SHA-256 of the previous 32 bytes; a draw below
//! `b` reads the next 4 bytes as a little-endian `u32` `r`, rejects `r ≥ 2^32 − (2^32 mod b)`,
//! and returns `r mod b`; for `i` from `n − 1` down to 1, entry `i` swaps with entry
//! `draw(i + 1)`.

use heartwood_crypto::identities::PublicKeyBytes;
use heartwood_crypto::{BlockHeader, Milestones};
use heartwood_kernel::RoundInfo;

use crate::errors::{RewardError, RoundError};
use crate::undo::RoundUndo;
use crate::wallets::{Delegate, WalletRepository};

/// A delegate of a round, as the round's rows hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundDelegate {
    /// The delegate's name.
    pub username: String,
    /// The delegate's public key.
    pub public_key: PublicKeyBytes,
    /// The delegate's vote balance at the round start.
    pub vote_balance: u64,
}

/// A slot in which the scheduled delegate forged no block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedBlock {
    /// The first second of the slot, since the network epoch.
    pub timestamp: i64,
    /// The height of the block that was forged after the missed slots.
    pub height: u32,
    /// The name of the delegate that missed the slot.
    pub username: String,
}

/// The reward of a block in its round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundReward {
    /// Whether the delegate has already forged a block in this round.
    pub already_forged: bool,
    /// The reward.
    pub reward: u64,
}

/// The state of the current round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundState {
    info: RoundInfo,
    delegates: Vec<RoundDelegate>,
    forging: Vec<RoundDelegate>,
    blocks: Vec<String>,
}

impl RoundState {
    /// Start the round `info` as the reference's `applyRound` does: rank the delegates, take the
    /// round's delegates and shuffle them into the forging list. No block of the round is forged
    /// yet.
    pub fn start(
        wallets: &mut WalletRepository,
        info: RoundInfo,
    ) -> Result<RoundState, RoundError> {
        let _ = (wallets, info);
        todo!()
    }

    /// The round whose delegates forge the next block.
    pub fn info(&self) -> &RoundInfo {
        &self.info
    }

    /// The round's delegates in rank order: the round rows the reference saves.
    pub fn round_delegates(&self) -> &[RoundDelegate] {
        &self.delegates
    }

    /// The forging list: the round's delegates in the order they forge.
    pub fn forging_delegates(&self) -> &[RoundDelegate] {
        &self.forging
    }

    /// The names of the delegates that forged the round's blocks so far, in block order.
    pub fn blocks(&self) -> &[String] {
        &self.blocks
    }

    /// Record the block at `height` forged by `username`; when it ends the round, start the next
    /// one and return what reverting the start needs.
    pub fn apply_block(
        &mut self,
        wallets: &mut WalletRepository,
        height: u32,
        username: &str,
        milestones: &Milestones,
    ) -> Result<Option<RoundUndo>, RoundError> {
        let _ = (wallets, height, username, milestones);
        todo!()
    }

    /// Take back the block at `height`, the last one recorded: restore the previous round and
    /// every rank from `undo` when the block started a round.
    pub fn revert_block(
        &mut self,
        wallets: &mut WalletRepository,
        height: u32,
        undo: Option<RoundUndo>,
    ) -> Result<(), RoundError> {
        let _ = (wallets, height, undo);
        todo!()
    }

    /// The slots missed between the last block `last` and the next block `next`, each with the
    /// delegate the forging list scheduled for it; none when `last` is the genesis block.
    pub fn detect_missed_blocks(
        &self,
        last: &BlockHeader,
        next: &BlockHeader,
        milestones: &Milestones,
    ) -> Vec<MissedBlock> {
        let _ = (last, next, milestones);
        todo!()
    }

    /// The reward of a block at `height` forged by `delegate`: the secondary reward when the
    /// dynamic reward is on and the delegate has already forged in this round, else the reward of
    /// its rank.
    pub fn reward_for_block_in_round(
        &self,
        height: u32,
        delegate: &Delegate,
        milestones: &Milestones,
    ) -> Result<RoundReward, RewardError> {
        let _ = (height, delegate, milestones);
        todo!()
    }
}

/// `delegates` in the forging order of round `round` (see the module documentation).
pub fn shuffle_delegates<T: Clone>(round: u64, delegates: &[T]) -> Vec<T> {
    let _ = (round, delegates);
    todo!()
}
