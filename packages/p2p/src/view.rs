//! What the client reads of the node, and what it tells the node: four traits that the node
//! implements over its published snapshots and its store.
//!
//! The P2P crate has no edge to the state, the pool or the database. It reads the chain through
//! [`ChainView`] (the reference's state store, wallet repository and configuration), stored blocks
//! and round delegates through [`BlockLookup`] (the reference's database interceptor), writes the
//! versions that status attestations prove through [`OnlineSink`] (the node-local record of online
//! delegates), and reports its peer events through [`PeerEvents`]. The tests fake all four.

use std::sync::Arc;

use heartwood_crypto::{Block, BlockHeader, BlockId, Milestones, Network, PublicKeyBytes};
use heartwood_kernel::{Event, RoundInfo};

use crate::errors::LookupError;

/// A registered delegate, as the client reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateInfo {
    /// The delegate's username.
    pub username: String,
    /// The delegate's rank, when it has one.
    pub rank: Option<u32>,
}

/// The last block ping: how often the node has been told of its last received block, and when.
/// A block whose ping came from a peer less than 500 ms ago is broadcast only after the rest of
/// the 500 ms, and not at all when the ping has moved to another height by then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPing {
    /// The block's id.
    pub id: BlockId,
    /// The block's height.
    pub height: u32,
    /// When the node first heard of the block, in milliseconds since the Unix epoch.
    pub first_ms: i64,
    /// When the node last heard of the block, in milliseconds since the Unix epoch.
    pub last_ms: i64,
    /// Whether the block came from the node's own forger.
    pub from_forger: bool,
}

/// The node's chain as the client reads it: the published snapshot after the last commit.
pub trait ChainView: Send + Sync {
    /// The last block.
    fn last_block(&self) -> Arc<Block>;

    /// The last blocks the node keeps in memory, at most 100, in height order.
    fn last_blocks(&self) -> Vec<Arc<Block>>;

    /// The milestones.
    fn milestones(&self) -> &Milestones;

    /// The network description: its name and nethash among others.
    fn network(&self) -> &Network;

    /// The registered delegate whose public key is `key`, if any.
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo>;

    /// The public keys of every registered delegate, in the order of the reference's
    /// `allByUsername`.
    fn delegate_keys(&self) -> Vec<PublicKeyBytes>;

    /// Whether the node is synced with the network.
    fn is_synced(&self) -> bool;

    /// Whether the node's state machine is idle; the reference then pauses for a millisecond
    /// after each downloaded transaction it reads, and the communicator does too, for at most
    /// half a second per reply.
    fn is_idle(&self) -> bool;

    /// The last block ping, if a block has been received.
    fn block_ping(&self) -> Option<BlockPing>;
}

/// Stored blocks and round delegates, as the reference's database interceptor gives them.
pub trait BlockLookup: Send + Sync {
    /// The headers of the blocks at `heights`, in the order asked; the in-memory last blocks are
    /// read before the store.
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockHeader>, LookupError>;

    /// The public keys of the active delegates of `round`, in the reference's `getActiveDelegates`
    /// order: the stored round's rows, or the current round's delegates when the round is not
    /// stored yet.
    fn round_delegate_keys(&self, round: RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError>;
}

/// The node-local record of online delegates, which status attestations update.
pub trait OnlineSink: Send + Sync {
    /// Record that the delegate with `key` attested `version` in `round`.
    fn set_version(&self, key: PublicKeyBytes, round: u64, version: String);
}

/// The peer events the client reports, by the reference's event names (`peer.added`,
/// `peer.removed`).
pub trait PeerEvents: Send + Sync {
    /// Report `event`.
    fn emit(&self, event: Event);
}

/// Peer events reported to no one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEvents;

impl PeerEvents for NoEvents {
    fn emit(&self, _event: Event) {}
}
