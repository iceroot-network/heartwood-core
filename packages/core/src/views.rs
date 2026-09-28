//! The node as the peer-to-peer client and the API read it, and what they tell it: the client's
//! [`ChainView`], [`BlockLookup`], [`OnlineSink`] and [`PeerEvents`], the API's
//! [`SubmitTransactions`] and [`PeerList`], and the node thread's listener and broadcast.
//!
//! The chain is read from the chain state the node thread shares with the blockchain service (its
//! last blocks, and the rounds a running job has started, change as the job runs, as the
//! reference's state store and its saved rounds do), the wallets and the round from the last
//! published state, stored blocks and rounds through the store's read-only connections. The
//! network monitor and the blockchain service are made after the node thread, which needs to
//! reach them, so they are linked in once made ([`Links`]); until then the node reads as not
//! synced and not idle, and nothing is broadcast.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use heartwood_api::context::{SubmitError, SubmitFuture};
use heartwood_api::{PeerList, SubmitTransactions};
use heartwood_blockchain::chain_state::{SharedChainState, lock};
use heartwood_blockchain::events::{NodeEvent, NodeListener};
use heartwood_blockchain::pool_task::TransactionBroadcast;
use heartwood_blockchain::state_machine::MachineState;
use heartwood_blockchain::{Blockchain, Channel, Network as NetworkCalls, NodeHandle, OnlineWork};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Block, BlockHeader, Milestones, Network, PublicKeyBytes, Signature, Transaction,
};
use heartwood_database::{BlockRow, Readers, RoundRow};
use heartwood_kernel::bus::EventBus;
use heartwood_kernel::{Event, RoundInfo};
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::monitor::PeerInfo;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};
use heartwood_p2p::view::{BlockPing, DelegateInfo};
use heartwood_p2p::{AttestedKeys, BlockLookup, ChainView, NetworkMonitor, OnlineSink, PeerEvents};
use heartwood_pool::Entry;

/// What the node reads of the network monitor.
pub trait Peers: Send + Sync {
    /// The peers, for the API.
    fn peers(&self) -> Vec<PeerInfo>;
    /// The median of the peers' heights.
    fn network_height(&self) -> u32;
    /// A fast forced cleanse, at most every 10 s.
    fn ping_all(&self);
    /// Disconnect the peers whose version no longer passes the gate.
    fn disconnect_invalid_peers(&self);
    /// Broadcast `transactions` to every peer.
    fn broadcast_transactions(&self, transactions: &[Transaction]);
}

impl<C, A, R> Peers for Arc<NetworkMonitor<C, A, R>>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn peers(&self) -> Vec<PeerInfo> {
        NetworkMonitor::peers(self)
    }
    fn network_height(&self) -> u32 {
        NetworkMonitor::network_height(self)
    }
    fn ping_all(&self) {
        NetworkMonitor::ping_all(self);
    }
    fn disconnect_invalid_peers(&self) {
        NetworkMonitor::disconnect_invalid_peers(self);
    }
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        NetworkMonitor::broadcast_transactions(self, transactions);
    }
}

/// What the node reads of the blockchain service's machine.
pub trait Machine: Send + Sync {
    /// Whether the tip is less than three block times old (true without peers).
    fn is_synced(&self) -> bool;
    /// Whether the machine is idle.
    fn is_idle(&self) -> bool;
}

impl<N: NetworkCalls> Machine for Arc<Blockchain<N>> {
    fn is_synced(&self) -> bool {
        Blockchain::is_synced(self, None)
    }
    fn is_idle(&self) -> bool {
        self.status().borrow().state == MachineState::Idle
    }
}

/// The network monitor, the machine and the forger's attestation session, linked in once made,
/// and what the server reads of the node's start: its boot and its first sync.
#[derive(Default)]
pub struct Links {
    /// The network monitor.
    pub peers: OnceLock<Arc<dyn Peers>>,
    /// The blockchain service's machine.
    pub machine: OnceLock<Arc<dyn Machine>>,
    /// The forger's attestation session: the delegates on this node.
    pub attested: OnceLock<Arc<dyn AttestedKeys>>,
    /// When the node first reported itself synced, in milliseconds since the Unix epoch.
    pub first_synced: OnceLock<i64>,
    /// Whether the blockchain has booted.
    pub booted: AtomicBool,
}

/// The delegates on this node: the attestation session's, none before the server is made.
impl AttestedKeys for Links {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        let session = self.attested.get();
        session
            .map(|session| session.attested())
            .unwrap_or_default()
    }
}

impl Links {
    /// Whether the node is synced; not before the machine is linked.
    pub fn synced(&self) -> bool {
        self.machine
            .get()
            .is_some_and(|machine| machine.is_synced())
    }

    fn with_peers(&self, call: impl FnOnce(&dyn Peers)) {
        if let Some(peers) = self.peers.get() {
            call(peers.as_ref());
        }
    }
}

/// The stored blocks and rounds the node reads through the store's read-only connections.
pub trait History: Send + Sync {
    /// The rows of the stored blocks at `heights`.
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockRow>, LookupError>;
    /// The rows of the stored `round`, in the reference's `getRound` order.
    fn round(&self, round: u64) -> Result<Vec<RoundRow>, LookupError>;
}

impl History for Readers {
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockRow>, LookupError> {
        self.get()
            .blocks_by_height(heights)
            .map_err(|e| LookupError(e.to_string()))
    }
    fn round(&self, round: u64) -> Result<Vec<RoundRow>, LookupError> {
        self.get()
            .round(round)
            .map_err(|e| LookupError(e.to_string()))
    }
}

/// The node, as the peer-to-peer client reads it.
pub struct NodeView {
    /// The node thread.
    pub handle: NodeHandle,
    /// The chain state shared with the node thread.
    pub chain: SharedChainState,
    /// The milestones.
    pub milestones: Arc<Milestones>,
    /// The network.
    pub network: Arc<Network>,
    /// The stored blocks and rounds.
    pub history: Arc<dyn History>,
    /// The monitor and the machine.
    pub links: Arc<Links>,
}

impl ChainView for NodeView {
    fn last_block(&self) -> Arc<Block> {
        let last = lock(&self.chain).last_block().cloned();
        last.unwrap_or_else(|| Arc::clone(&self.handle.state().borrow().last_block))
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        lock(&self.chain).last_blocks.iter().cloned().collect()
    }
    fn milestones(&self) -> &Milestones {
        &self.milestones
    }
    fn network(&self) -> &Network {
        &self.network
    }
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        let state = Arc::clone(&self.handle.state().borrow());
        let delegate = state.wallets.find_by_public_key(key)?.delegate()?;
        Some(DelegateInfo {
            username: delegate.username().to_owned(),
            rank: delegate.rank,
        })
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        let state = Arc::clone(&self.handle.state().borrow());
        state
            .wallets
            .delegates()
            .filter_map(|wallet| Some(wallet.public_key()?.to_compressed()))
            .collect()
    }
    fn is_synced(&self) -> bool {
        self.links.synced()
    }
    fn is_idle(&self) -> bool {
        self.links
            .machine
            .get()
            .is_some_and(|machine| machine.is_idle())
    }
    fn block_ping(&self) -> Option<BlockPing> {
        lock(&self.chain)
            .ping
            .as_ref()
            .map(|ping| ping.ping.clone())
    }
}

impl BlockLookup for NodeView {
    fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockHeader>, LookupError> {
        let memory = self.last_blocks();
        let in_memory = |height: &u32| memory.iter().find(|block| block.height() == *height);
        let stored: Vec<u32> = heights
            .iter()
            .filter(|h| in_memory(h).is_none())
            .copied()
            .collect();
        let rows = match stored.is_empty() {
            true => Vec::new(),
            false => self.history.blocks_by_height(&stored)?,
        };
        let from_rows = |height: &u32| rows.iter().find(|row| row.height == *height).map(header);
        let find = |height: &u32| match in_memory(height) {
            Some(block) => Some(Ok(block.header().clone())),
            None => from_rows(height),
        };
        heights.iter().filter_map(find).collect()
    }

    /// The delegates of `round`: a round the running job started, as the chain state recorded it
    /// with the tip; else the stored round; else the published state's round, only when it is
    /// the one asked, as the reference asserts before its fallback.
    fn round_delegate_keys(&self, round: RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError> {
        let (asked, chain) = (round.round, lock(&self.chain));
        let started = chain
            .rounds_started
            .iter()
            .find(|(number, _)| *number == asked);
        if let Some((_, keys)) = started {
            return Ok(keys.clone());
        }
        drop(chain);
        let rows = self.history.round(asked)?;
        if rows.is_empty() {
            let state = Arc::clone(&self.handle.state().borrow());
            let current = state.round.info().round;
            if current != asked {
                let why =
                    format!("round {asked} is not stored, and the current round is {current}");
                return Err(LookupError(why));
            }
            return Ok(state
                .round
                .round_delegates()
                .iter()
                .map(|delegate| delegate.public_key)
                .collect());
        }
        let key =
            |text: &str| PublicKeyBytes::from_hex(text).map_err(|e| LookupError(e.to_string()));
        rows.iter().map(|row| key(&row.public_key)).collect()
    }
}

/// The header of a stored block's row.
pub(crate) fn header(row: &BlockRow) -> Result<BlockHeader, LookupError> {
    let fail = |detail: String| {
        LookupError(format!(
            "the stored block at height {}: {detail}",
            row.height
        ))
    };
    Ok(BlockHeader {
        version: row.version,
        timestamp: row.timestamp,
        height: row.height,
        previous_block: row.previous_block.map_or([0; 32], |id| *id.as_bytes()),
        number_of_transactions: row.number_of_transactions,
        total_amount: row.total_amount,
        total_fee: row.total_fee,
        reward: row.reward,
        payload_length: row.payload_length,
        payload_hash: hex::decode_array(&row.payload_hash).map_err(|e| fail(e.to_string()))?,
        generator_public_key: PublicKeyBytes::from_hex(&row.generator_public_key)
            .map_err(|e| fail(e.to_string()))?,
        block_signature: Signature::from_hex(&row.block_signature)
            .map_err(|e| fail(e.to_string()))?,
    })
}

impl OnlineSink for NodeView {
    fn set_version(&self, key: PublicKeyBytes, round: u64, version: String) {
        // A stopped node takes no more work; the record is then of no use.
        let _ = self.handle.online(OnlineWork::Set {
            key,
            round,
            version,
        });
    }
}

/// The node's events on the kernel's bus, and its internal listeners as ordered calls: `pingAll`
/// at each applied round while synced, the peers of an invalid version disconnected at a
/// milestone change.
pub struct Listener {
    /// The bus.
    pub bus: EventBus,
    /// The monitor and the machine.
    pub links: Arc<Links>,
}

impl NodeListener for Listener {
    fn committed(&self, events: &[NodeEvent]) {
        for event in events {
            self.bus.emit(event.event());
            match event {
                NodeEvent::BlockApplied {
                    round_started: Some(_),
                    ..
                } => {
                    self.bus.emit(Event::RoundApplied);
                    if self.links.synced() {
                        self.links.with_peers(|peers| peers.ping_all());
                    }
                }
                NodeEvent::MilestoneChanged { .. } => self
                    .links
                    .with_peers(|peers| peers.disconnect_invalid_peers()),
                _ => {}
            }
        }
    }
}

impl TransactionBroadcast for Links {
    fn broadcast_transactions(&self, transactions: &[Transaction]) {
        self.with_peers(|peers| peers.broadcast_transactions(transactions));
    }
}

impl PeerList for Links {
    fn peers(&self) -> Vec<PeerInfo> {
        self.peers
            .get()
            .map(|peers| peers.peers())
            .unwrap_or_default()
    }
}

/// The peer events on the kernel's bus.
pub struct BusEvents(pub EventBus);

impl PeerEvents for BusEvents {
    fn emit(&self, event: Event) {
        self.0.emit(event);
    }
}

/// `POST /transactions`: the entries to the node thread's API processor.
pub struct Submit(pub NodeHandle);

impl SubmitTransactions for Submit {
    fn submit(&self, entries: Vec<Entry>) -> SubmitFuture {
        let handle = self.0.clone();
        Box::pin(async move {
            handle
                .pool_entries(Channel::Api, entries)
                .await
                .map_err(|_| SubmitError)
        })
    }
}
