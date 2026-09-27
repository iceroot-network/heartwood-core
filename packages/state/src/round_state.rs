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

use std::collections::BTreeMap;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::utils::reward_calculator::calculate_reward;
use heartwood_crypto::{BlockHeader, Milestones};
use heartwood_kernel::{RoundInfo, calculate_round, is_new_round};

use crate::dpos::dpos::set_rank;
use crate::dpos::{build_delegate_ranking, set_delegates_round};
use crate::errors::{RewardError, RoundError};
use crate::persist::{RestoreError, RoundDelegateRecord, RoundRecord, amount, read};
use crate::undo::RoundUndo;
use crate::wallets::{Delegate, Wallet, WalletRepository};

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
    /// yet. On an error every rank is as it was.
    pub fn start(
        wallets: &mut WalletRepository,
        info: RoundInfo,
    ) -> Result<RoundState, RoundError> {
        let ranks = rank_snapshot(wallets);
        RoundState::start_or_restore(wallets, info, &ranks)
    }

    /// Start the round `info`; on an error, put back the ranks of `ranks`, taken before.
    fn start_or_restore(
        wallets: &mut WalletRepository,
        info: RoundInfo,
        ranks: &[(Address, Option<u32>)],
    ) -> Result<RoundState, RoundError> {
        let delegates = build_delegate_ranking(wallets)
            .and_then(|ranked| set_delegates_round(wallets, &ranked, &info));
        match delegates {
            Ok(delegates) => Ok(RoundState {
                info,
                forging: shuffle_delegates(info.round, &delegates),
                delegates,
                blocks: Vec::new(),
            }),
            Err(error) => {
                restore_ranks(wallets, ranks);
                Err(error)
            }
        }
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
    /// one and return what reverting the start needs. On an error the round state and every rank
    /// are as they were.
    pub fn apply_block(
        &mut self,
        wallets: &mut WalletRepository,
        height: u32,
        username: &str,
        milestones: &Milestones,
    ) -> Result<Option<RoundUndo>, RoundError> {
        self.blocks.push(username.to_owned());
        // A block at height 2^32 − 1 is the last one there can be, so it starts no round.
        let next = match height.checked_add(1) {
            Some(next) if height == 1 || is_new_round(next, milestones) => next,
            _ => return Ok(None),
        };
        let ranks = rank_snapshot(wallets);
        match RoundState::start_or_restore(wallets, calculate_round(next, milestones), &ranks) {
            Ok(started) => {
                let previous = std::mem::replace(self, started);
                Ok(Some(RoundUndo { previous, ranks }))
            }
            Err(error) => {
                self.blocks.pop();
                Err(error)
            }
        }
    }

    /// Take back the block at `height`, the last one recorded: restore the previous round and
    /// every rank from `undo` when the block started a round. On an error nothing changes.
    pub fn revert_block(
        &mut self,
        wallets: &mut WalletRepository,
        height: u32,
        undo: Option<RoundUndo>,
    ) -> Result<(), RoundError> {
        let round = undo.as_ref().map_or(&*self, |undo| &undo.previous);
        if !round.ends_with(height) {
            return Err(RoundError::NotLastBlock { height });
        }
        if let Some(RoundUndo { previous, ranks }) = undo {
            restore_ranks(wallets, &ranks);
            *self = previous;
        }
        self.blocks.pop();
        Ok(())
    }

    /// Whether the recorded blocks are those of the round up to `height`: one for each height
    /// from the round's first height (the genesis block, which the round does not record,
    /// excepted) to `height`.
    fn ends_with(&self, height: u32) -> bool {
        let first = self.info.round_height.max(2);
        u64::from(height)
            .checked_add(1)
            .and_then(|end| end.checked_sub(first))
            .is_some_and(|count| count >= 1 && u64::try_from(self.blocks.len()) == Ok(count))
    }

    /// The slots missed between the last block `last` and the next block `next`, each with the
    /// delegate the forging list scheduled for it; none when `last` is the genesis block.
    ///
    /// There is one row per slot between the two blocks, so the caller refuses a block whose
    /// timestamp lies in the future before it asks.
    pub fn detect_missed_blocks(
        &self,
        last: &BlockHeader,
        next: &BlockHeader,
        milestones: &Milestones,
    ) -> Vec<MissedBlock> {
        if last.height == 1 || self.forging.is_empty() {
            return Vec::new();
        }
        let block_time = milestones.at(1).block_time();
        let last_slot = slots::slot_number(i64::from(last.timestamp), block_time);
        let next_slot = slots::slot_number(i64::from(next.timestamp), block_time);
        let slot_time = slots::slot_time(last_slot, block_time);
        let step = i64::try_from(block_time).unwrap_or(i64::MAX);
        let count = i64::try_from(self.forging.len()).unwrap_or(i64::MAX);
        let mut missed = Vec::new();
        let mut timestamp = slot_time;
        let mut slot = last_slot;
        loop {
            slot = slot.saturating_add(1);
            timestamp = timestamp.saturating_add(step);
            if slot >= next_slot {
                return missed;
            }
            let index = usize::try_from(slot.rem_euclid(count)).ok();
            if let Some(delegate) = index.and_then(|index| self.forging.get(index)) {
                missed.push(MissedBlock {
                    timestamp,
                    height: next.height,
                    username: delegate.username.clone(),
                });
            }
        }
    }

    /// The reward of a block at `height` forged by `delegate`: the secondary reward when the
    /// dynamic reward is on and the delegate has already forged in this round, else the reward of
    /// its rank. Without a rank, only the dynamic reward table has no answer.
    pub fn reward_for_block_in_round(
        &self,
        height: u32,
        delegate: &Delegate,
        milestones: &Milestones,
    ) -> Result<RoundReward, RewardError> {
        let params = milestones.at(height);
        let dynamic = params.dynamic_reward().filter(|table| table.enabled());
        let already_forged =
            dynamic.is_some() && self.blocks.iter().any(|name| name == delegate.username());
        let reward = match (dynamic, delegate.rank) {
            (Some(table), _) if already_forged => table.secondary_reward(),
            (Some(_), None) => {
                return Err(RewardError::NoRank {
                    username: delegate.username().to_owned(),
                });
            }
            // Without the dynamic reward, the rank is not read.
            (_, rank) => calculate_reward(params, rank.unwrap_or_default())?,
        };
        Ok(RoundReward {
            already_forged,
            reward,
        })
    }
}

/// The round state's record in the store (see [`crate::persist`]).
impl RoundState {
    /// The round state's record.
    pub fn to_record(&self) -> RoundRecord {
        let delegates = self.delegates.iter().map(|delegate| RoundDelegateRecord {
            u: delegate.username.clone(),
            pk: delegate.public_key.to_hex(),
            vb: delegate.vote_balance.to_string(),
        });
        RoundRecord {
            round: self.info.round,
            rh: self.info.round_height,
            nr: self.info.next_round,
            md: self.info.max_delegates,
            d: delegates.collect(),
            f: self.forging.iter().map(|d| d.public_key.to_hex()).collect(),
            b: self.blocks.clone(),
        }
    }

    /// The round state of `record`, whose forging list must be the shuffle of the round's
    /// delegates.
    pub fn from_record(record: RoundRecord) -> Result<RoundState, RestoreError> {
        let mut delegates = Vec::with_capacity(record.d.len());
        for d in record.d {
            delegates.push(RoundDelegate {
                public_key: read("round", PublicKeyBytes::from_hex(&d.pk))?,
                vote_balance: amount("round", &d.vb)?,
                username: d.u,
            });
        }
        let forging = shuffle_delegates(record.round, &delegates);
        if forging.iter().map(|d| d.public_key.to_hex()).ne(record.f) {
            return Err(RestoreError::Forging);
        }
        Ok(RoundState {
            info: RoundInfo {
                round: record.round,
                round_height: record.rh,
                next_round: record.nr,
                max_delegates: record.md,
            },
            delegates,
            forging,
            blocks: record.b,
        })
    }
}

/// Every registered delegate's rank.
fn rank_snapshot(wallets: &WalletRepository) -> Vec<(Address, Option<u32>)> {
    wallets
        .delegates()
        .filter_map(|wallet| Some((*wallet.address(), wallet.delegate()?.rank)))
        .collect()
}

/// Give every registered delegate its rank of `ranks`, and no rank to a delegate `ranks` does
/// not list.
fn restore_ranks(wallets: &mut WalletRepository, ranks: &[(Address, Option<u32>)]) {
    let ranks: BTreeMap<Address, Option<u32>> = ranks.iter().copied().collect();
    let delegates: Vec<Address> = wallets.delegates().map(Wallet::address).copied().collect();
    for address in &delegates {
        set_rank(wallets, address, ranks.get(address).copied().flatten());
    }
}

/// `delegates` in the forging order of round `round` (see the module documentation).
pub fn shuffle_delegates<T: Clone>(round: u64, delegates: &[T]) -> Vec<T> {
    let mut order = delegates.to_vec();
    let mut stream = Stream::new(round);
    for i in (1..order.len()).rev() {
        let bound = u64::try_from(i).map_or(u64::MAX, |i| i.saturating_add(1));
        let j = usize::try_from(stream.draw(bound)).unwrap_or(i);
        order.swap(i, j.min(i));
    }
    order
}

/// The shuffle's byte stream, read four bytes at a time.
struct Stream {
    block: [u8; 32],
    next: usize,
}

impl Stream {
    /// The 4-byte words of a 32-byte block.
    const WORDS: usize = 8;

    fn new(round: u64) -> Stream {
        Stream {
            block: sha256(round.to_string().as_bytes()),
            next: 0,
        }
    }

    /// The next 4 bytes as a little-endian `u32`; after the 32 bytes of a block, the next block is
    /// the SHA-256 of these 32 bytes.
    fn word(&mut self) -> u32 {
        if self.next >= Stream::WORDS {
            self.block = sha256(&self.block);
            self.next = 0;
        }
        let (words, _) = self.block.as_chunks::<4>();
        let word = words.get(self.next).copied().map_or(0, u32::from_le_bytes);
        self.next = self.next.saturating_add(1);
        word
    }

    /// A number below `bound` (1 or more), without bias. A bound above `2^32` cannot arise (it
    /// would need more than `2^32` delegates); it takes the word without rejection.
    fn draw(&mut self, bound: u64) -> u64 {
        const SPAN: u64 = 1 << 32;
        let bound = bound.max(1);
        let limit = SPAN.saturating_sub(SPAN.checked_rem(bound).unwrap_or_default());
        loop {
            let word = u64::from(self.word());
            if word < limit || limit == 0 {
                return word.checked_rem(bound).unwrap_or_default();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(round: u64, count: usize) -> Vec<usize> {
        let input: Vec<usize> = (0..count).collect();
        shuffle_delegates(round, &input)
    }

    fn golden(text: &str) -> Vec<usize> {
        text.split(',').map(|n| n.parse().unwrap()).collect()
    }

    #[test]
    fn the_shuffle_gives_the_golden_orders() {
        assert_eq!(
            order(1, 53),
            golden(
                "8,15,41,21,36,47,40,50,38,10,35,16,20,31,4,48,1,18,34,0,25,46,42,9,3,22,17,13,39,\
                 28,45,33,30,6,44,7,26,51,24,43,19,37,12,29,32,49,14,2,23,5,11,52,27"
            )
        );
        assert_eq!(order(1, 0), Vec::<usize>::new());
        assert_eq!(order(1, 1), [0]);
        assert_eq!(order(7, 2), [0, 1]);
        assert_eq!(order(7, 3), [1, 0, 2]);
    }

    #[test]
    fn a_rejected_draw_reads_the_next_word() {
        // Round 1,054,879: for i = 51 (bound 52) the stream's bytes 4 to 7 are rejected.
        assert_eq!(
            order(1_054_879, 53),
            golden(
                "17,7,10,1,12,14,35,11,8,40,26,20,3,28,18,16,4,43,5,27,31,48,29,15,39,34,42,32,49,\
                 44,41,38,45,24,13,46,50,33,37,51,21,2,25,47,30,9,0,36,19,52,6,22,23"
            )
        );
    }

    #[test]
    fn the_stream_continues_with_the_hash_of_the_previous_block() {
        let seed = sha256(b"5");
        let mut stream = Stream::new(5);
        for _ in 0..8 {
            stream.word();
        }
        let next = sha256(&seed);
        assert_eq!(
            stream.word(),
            u32::from_le_bytes([next[0], next[1], next[2], next[3]])
        );
    }
}
