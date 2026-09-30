//! Heartwood's side of a lockstep chain: its state, its pool with its processor and its online
//! record, driven as a synced node drives them.

use std::sync::Arc;

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::{Aux, Block, BlockId, KeyPair, Milestones, Network, Transaction};
use heartwood_forger::{Delegate, ForgeInputs};
use heartwood_kernel::calculate_round;
use heartwood_kernel::clock::ManualClock;
use heartwood_kernel::config::crypto::CryptoFiles;
use heartwood_kernel::utils::calculate_forging_info::calculate_forging_info;
use heartwood_kernel::utils::round_calculator::is_new_round;
use heartwood_pool::{DelegateVersions, Entry, Pool, PoolContext, Processor, Reindexed, collate};
use heartwood_state::State;
use heartwood_state::digest::{self, Step};
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use super::LockstepError;
use crate::digests;
use crate::levels::pool::{VERSION, options};
use crate::store::ScratchDir;

fn heartwood(what: &str, e: impl std::fmt::Display) -> LockstepError {
    LockstepError::Heartwood(format!("{what}: {e}"))
}

/// What Heartwood did with a block it applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    /// The digest line of the state after the block.
    pub line: Value,
    /// The round whose start the block ended: the online record is expired at it.
    pub expire_at: Option<u64>,
    /// The slots missed before the block.
    pub missed: usize,
    /// The number of transactions of the block.
    pub transactions: usize,
}

/// A slot's delegate and the timestamp of the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// The delegate's name.
    pub username: String,
    /// The delegate's public key.
    pub public_key: PublicKeyBytes,
    /// The slot's timestamp, in seconds since the epoch.
    pub timestamp: u32,
}

/// Heartwood's state, pool and online record.
pub struct Heart {
    /// The milestones.
    pub milestones: Milestones,
    /// The network.
    pub network: Network,
    /// The state at the tip.
    pub state: State,
    pool: Pool,
    processor: Processor,
    online: DelegateVersions,
    _scratch: ScratchDir,
}

/// The digest line of Heartwood's state, as the oracle's is compared with it: without the chain
/// digest, which chains through the lines of a whole replay.
pub fn comparable(line: &Value) -> Value {
    let mut line = line.clone();
    if let Some(o) = line.as_object_mut() {
        o.shift_remove("cd");
    }
    line
}

impl Heart {
    /// Heartwood's state on `genesis`, and an empty pool with the default options.
    pub fn open(files: &CryptoFiles, genesis: &Block) -> Result<Heart, LockstepError> {
        let scratch =
            ScratchDir::new("lockstep").map_err(|e| heartwood("a scratch directory", e))?;
        let state = State::from_genesis(genesis, &files.milestones, &Handlers)
            .map_err(|e| heartwood("the genesis block", e))?;
        let options = options(&json!({}), scratch.path()).map_err(|e| heartwood("the pool", e))?;
        let pool = Pool::open(options, None).map_err(|e| heartwood("the pool", e))?;
        Ok(Heart {
            milestones: files.milestones.clone(),
            network: files.network.clone(),
            state,
            pool,
            processor: Processor::new(),
            online: DelegateVersions::new(),
            _scratch: scratch,
        })
    }

    /// The digest line of the genesis state.
    pub fn genesis_line(&self) -> Value {
        digests::line_value(&digest::height_digest(
            &self.state,
            &self.milestones,
            Step::Genesis,
            &[],
            None,
            &[0; 32],
        ))
    }

    /// The tip's height.
    pub fn height(&self) -> u32 {
        self.state.height()
    }

    fn network_byte(&self) -> u8 {
        self.network.pub_key_hash
    }

    /// The nonce of the wallet at `address` in the state.
    pub fn nonce(&self, address: &Address) -> u64 {
        self.state
            .wallets()
            .find_by_address(address)
            .map_or(0, heartwood_state::Wallet::nonce)
    }

    /// The public key of the delegate `name` in the state's wallets.
    pub fn delegate_key(&self, name: &str) -> Option<PublicKeyBytes> {
        self.state
            .wallets()
            .find_by_username(name)
            .and_then(|w| w.public_key())
            .map(heartwood_crypto::identities::PublicKey::to_compressed)
    }

    /// The public keys of the delegates that forge the current round.
    pub fn forging_keys(&self) -> Vec<PublicKeyBytes> {
        self.state
            .forging_delegates()
            .iter()
            .map(|d| d.public_key)
            .collect()
    }

    /// Record `keys` as online in `round`.
    pub fn set_online(&mut self, round: u64, keys: &[PublicKeyBytes]) {
        for key in keys {
            self.online.set(*key, round, VERSION.to_owned());
        }
    }

    /// The batch `entries` at `now` milliseconds through the processor: its result as JSON.
    pub fn submit(&mut self, now: i64, entries: Vec<Vec<u8>>) -> Result<Value, LockstepError> {
        let reindexed = Reindexed::default();
        let ctx = PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: &self.milestones,
            network: &self.network,
            online: &self.online,
            reindexed: &reindexed,
        };
        let batch = entries.into_iter().map(Entry::Bytes).collect();
        let result = self
            .processor
            .process(&mut self.pool, &ctx, &ManualClock::new(now), batch)
            .map_err(|e| heartwood("the processor", e))?;
        self.state.reindex_parties(reindexed.take());
        Ok(result.to_json())
    }

    /// The transactions of the next block, from the pool's collation at the tip plus one.
    pub fn candidates(&mut self) -> Result<Vec<Transaction>, LockstepError> {
        let reindexed = Reindexed::default();
        let ctx = PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: &self.milestones,
            network: &self.network,
            online: &self.online,
            reindexed: &reindexed,
        };
        let collated =
            collate(&mut self.pool, &ctx, true, &[]).map_err(|e| heartwood("the collation", e))?;
        self.state.reindex_parties(reindexed.take());
        Ok(collated.candidates)
    }

    /// The first slot at least `gap` slots after the next one whose delegate `stopped` does not
    /// stop, or an error when there is none within 200 slots.
    pub fn slot(
        &self,
        gap: u32,
        stopped: impl Fn(&str, &PublicKeyBytes) -> bool,
    ) -> Result<Slot, LockstepError> {
        let last = self.state.last_block().header().clone();
        let height = last.height + 1;
        let block_time = u32::try_from(self.milestones.at(height).block_time()).unwrap_or(8);
        for extra in gap..gap + 200 {
            let timestamp = last.timestamp + block_time * (1 + extra);
            let info = calculate_forging_info(i64::from(timestamp), height, &self.milestones);
            let Some(delegate) = usize::try_from(info.current_forger)
                .ok()
                .and_then(|i| self.state.forging_delegates().get(i))
            else {
                continue;
            };
            if stopped(&delegate.username, &delegate.public_key) {
                continue;
            }
            return Ok(Slot {
                username: delegate.username.clone(),
                public_key: delegate.public_key,
                timestamp,
            });
        }
        Err(LockstepError::Recipe(format!(
            "every delegate of the slots after height {} is stopped",
            last.height
        )))
    }

    /// The block of `transactions` in `slot`, forged by `key` with `aux`.
    pub fn forge(
        &self,
        slot: &Slot,
        key: Arc<KeyPair>,
        aux: [u8; 32],
        transactions: &[Transaction],
    ) -> Result<Block, LockstepError> {
        let height = self.state.height();
        let reward = self
            .state
            .reward_for_block_in_round(height + 1, &slot.public_key, &self.milestones)
            .map(|r| r.reward)
            .map_err(|e| heartwood("the reward", e))?;
        let inputs = ForgeInputs {
            transactions,
            previous_id: BlockId::from_array(*self.state.last_block().id().as_bytes()),
            previous_height: height,
            timestamp: slot.timestamp,
            reward,
        };
        Delegate::new(key, self.network_byte())
            .forge(&inputs, Aux::fixed(aux), &self.milestones)
            .map_err(|e| heartwood("the forger", e))
    }

    /// Apply the block `bytes` as `step` and run a synced node's pool steps after it: the online
    /// record expired at a round start, the forged transactions removed, the clean-up and the
    /// re-add.
    pub fn apply(&mut self, bytes: &[u8], step: Step) -> Result<Posted, LockstepError> {
        let height = self.state.height() + 1;
        let refused = |detail: String| LockstepError::Block { height, detail };
        let block = Block::receive(bytes, &self.milestones)
            .map_err(|e| refused(format!("Heartwood cannot read its block: {e}")))?;
        let failures = block.verify(&self.milestones, i64::from(u32::MAX)).failures;
        if !failures.is_empty() {
            return Err(refused(format!(
                "Heartwood's block fails its checks: {failures:?}"
            )));
        }
        let header = block.header();
        let reward = self
            .state
            .reward_for_block_in_round(
                header.height,
                &header.generator_public_key,
                &self.milestones,
            )
            .ok()
            .map(|r| r.reward);
        let applied = self
            .state
            .apply_block(&block, &self.milestones, &Handlers)
            .map_err(|e| refused(format!("Heartwood refuses its block: {e}")))?;
        let line = digests::line_value(&digest::height_digest(
            &self.state,
            &self.milestones,
            step,
            &applied.missed,
            reward,
            &[0; 32],
        ));
        let expire_at = is_new_round(height + 1, &self.milestones)
            .then(|| calculate_round(height + 1, &self.milestones).round);
        if let Some(round) = expire_at {
            self.online.expire(round);
        }
        let network = self.network_byte();
        let reindexed = Reindexed::default();
        let ctx = PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: &self.milestones,
            network: &self.network,
            online: &self.online,
            reindexed: &reindexed,
        };
        for t in block.transactions() {
            let sender = Address::from_public_key(&t.data().sender_public_key, network);
            self.pool
                .remove_forged_transaction(&sender, t.id())
                .map_err(|e| heartwood("the pool", e))?;
        }
        self.pool
            .clean_up(&ctx)
            .map_err(|e| heartwood("the pool", e))?;
        self.pool
            .readd(&ctx, &[])
            .map_err(|e| heartwood("the pool", e))?;
        self.state.reindex_parties(reindexed.take());
        Ok(Posted {
            line,
            expire_at,
            missed: applied.missed.len(),
            transactions: block.transactions().len(),
        })
    }

    /// Revert `count` blocks, then offer their transactions to the pool again, in chain order:
    /// the revert lines and the blocks, the last one first.
    pub fn revert(&mut self, count: u32) -> Result<(Vec<Value>, Vec<Block>), LockstepError> {
        let (mut lines, mut blocks) = (Vec::new(), Vec::new());
        for _ in 0..count {
            let height = self.state.height();
            let reverted = self
                .state
                .revert_block(&self.milestones, &Handlers)
                .map_err(|e| LockstepError::Block {
                    height,
                    detail: format!("Heartwood cannot revert: {e}"),
                })?;
            blocks.push(reverted.block);
            lines.push(digests::line_value(&digest::height_digest(
                &self.state,
                &self.milestones,
                Step::Revert,
                &[],
                None,
                &[0; 32],
            )));
        }
        let forged: Vec<Transaction> = blocks
            .iter()
            .rev()
            .flat_map(|b| b.transactions().iter().cloned())
            .collect();
        let reindexed = Reindexed::default();
        let ctx = PoolContext {
            wallets: self.state.wallets(),
            tip: self.state.height(),
            milestones: &self.milestones,
            network: &self.network,
            online: &self.online,
            reindexed: &reindexed,
        };
        self.pool
            .readd(&ctx, &forged)
            .map_err(|e| heartwood("the pool", e))?;
        self.state.reindex_parties(reindexed.take());
        Ok((lines, blocks))
    }
}
