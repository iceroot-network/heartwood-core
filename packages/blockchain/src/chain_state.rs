//! What the reference keeps in its state store besides the state: the last blocks in memory, the
//! last downloaded block, the block ping, the forked block, the rollback count, the counters of
//! empty downloads and peer updates, and the start flags.
//!
//! The node thread and the blockchain service share one [`ChainState`] ([`SharedChainState`]):
//! the processor and the job change it while a job runs, the service's actions between jobs. Each
//! side holds the lock only for a step, never across an `.await`. The fields are plain, with the
//! reference's setters where they do more than set a field.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use heartwood_crypto::{Block, BlockId};
use heartwood_kernel::ChainLink;
use heartwood_p2p::view::BlockPing;

/// How many blocks the node keeps in memory: the reference's `storage.maxLastBlocks`.
pub const MAX_LAST_BLOCKS: usize = 100;

/// The last block ping, and how many times the block was received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ping {
    /// The block and the times of its first and last receipt.
    pub ping: BlockPing,
    /// How many times it was received (0 for a block from the node's own forger).
    pub count: u32,
}

/// The chain state of the reference's state store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainState {
    /// The last blocks, at most [`MAX_LAST_BLOCKS`], in height order; the last is the tip.
    pub last_blocks: VecDeque<Arc<Block>>,
    /// The last downloaded block: the tip of what the node has downloaded or accepted.
    pub last_downloaded: Option<ChainLink>,
    /// The last block ping.
    pub ping: Option<Ping>,
    /// The block of a detected fork, until the node is back at its height.
    pub forked_block: Option<Arc<Block>>,
    /// How many blocks the next fork recovery removes; 0 for a random count.
    pub blocks_to_rollback: u32,
    /// How many downloads in a row returned nothing.
    pub no_block_counter: u32,
    /// How many peer updates ran without a new block.
    pub p2p_update_counter: u32,
    /// Whether the blockchain has started (the machine reached idle once).
    pub started: bool,
    /// Whether the node starts a new network (`--networkStart`).
    pub network_start: bool,
    /// The height of the last stored block.
    pub last_stored_height: u32,
    /// When the processor last accepted a block, in milliseconds of the node's clock: the
    /// wake-up timer runs from it, as the reference restarts the timer at each accepted block.
    pub last_accepted_ms: Option<i64>,
}

/// The chain state, shared by the node thread and the blockchain service.
pub type SharedChainState = Arc<Mutex<ChainState>>;

/// Lock `shared`; a lock poisoned by a panic still gives the state.
pub fn lock(shared: &SharedChainState) -> MutexGuard<'_, ChainState> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ChainState {
    /// A chain state whose tip is `tip`, as a start leaves it: the tip in memory, and the last
    /// downloaded and stored block at the tip.
    pub fn at(tip: Arc<Block>) -> ChainState {
        let mut chain = ChainState {
            last_downloaded: Some(ChainLink::of(&tip)),
            last_stored_height: tip.height(),
            ..ChainState::default()
        };
        chain.last_blocks.push_back(tip);
        chain
    }

    /// The tip, once a block is in memory.
    pub fn last_block(&self) -> Option<&Arc<Block>> {
        self.last_blocks.back()
    }

    /// The tip's height (0 before a block is in memory).
    pub fn last_height(&self) -> u32 {
        self.last_block().map_or(0, |block| block.height())
    }

    /// Make `block` the tip, as the reference's `setLastBlock` does: blocks at or above its
    /// height go (a rollback), the oldest goes when more than [`MAX_LAST_BLOCKS`] are kept, and
    /// both counters restart.
    pub fn set_last_block(&mut self, block: Arc<Block>) {
        let height = block.height();
        while self
            .last_block()
            .is_some_and(|last| last.height() >= height)
        {
            self.last_blocks.pop_back();
        }
        self.last_blocks.push_back(block);
        while self.last_blocks.len() > MAX_LAST_BLOCKS {
            self.last_blocks.pop_front();
        }
        self.no_block_counter = 0;
        self.p2p_update_counter = 0;
    }

    /// The last blocks with heights from `start` to `end`, both included, in height order.
    pub fn last_blocks_by_height(&self, start: u32, end: u32) -> Vec<Arc<Block>> {
        let within = |block: &&Arc<Block>| (start..=end).contains(&block.height());
        self.last_blocks.iter().filter(within).cloned().collect()
    }

    /// The ids of the last blocks, the tip first, as the reference's `getLastBlockIds` lists them.
    pub fn last_block_ids(&self) -> Vec<BlockId> {
        self.last_blocks
            .iter()
            .rev()
            .map(|block| *block.id())
            .collect()
    }

    /// Set the last downloaded block to the tip, as the reference's
    /// `resetLastDownloadedBlock` does.
    pub fn reset_last_downloaded(&mut self) {
        self.last_downloaded = self.last_block().map(|block| ChainLink::of(block));
    }

    /// Record another receipt of `block` at `now_ms`, when it is the pinged block; whether it
    /// was, as the reference's `pingBlock` answers.
    pub fn ping_block(&mut self, id: &BlockId, height: u32, now_ms: i64) -> bool {
        match &mut self.ping {
            Some(ping) if ping.ping.id == *id && ping.ping.height == height => {
                ping.count = ping.count.saturating_add(1);
                ping.ping.last_ms = now_ms;
                true
            }
            _ => false,
        }
    }

    /// Start pinging `block`, received at `now_ms`, as the reference's `pushPingBlock` does; the
    /// previous ping, which the reference logs, is returned.
    pub fn push_ping_block(
        &mut self,
        id: BlockId,
        height: u32,
        from_forger: bool,
        now_ms: i64,
    ) -> Option<Ping> {
        let ping = Ping {
            ping: BlockPing {
                id,
                height,
                first_ms: now_ms,
                last_ms: now_ms,
                from_forger,
            },
            count: u32::from(!from_forger),
        };
        self.ping.replace(ping)
    }

    /// Forget the forked block.
    pub fn clear_forked_block(&mut self) {
        self.forked_block = None;
    }
}
