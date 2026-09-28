//! The node as the peer-to-peer server reads it and asks it for work: [`NodeServices`] over the
//! node thread's handle, its snapshots, the chain state, the store's read-only connections of the
//! server and the blockchain service, and [`PeerSide`] over the network monitor.

use std::net::IpAddr;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use heartwood_blockchain::NodeHandle;
use heartwood_crypto::{Block, BlockHeader, BlockId, PublicKeyBytes, TransactionId};
use heartwood_database::Readers;
use heartwood_kernel::{ChainLink, Clock};
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::monitor::CleanseOptions;
use heartwood_p2p::node_services::{Collation, ForgerEvent, NodeUnavailable, QueueState};
use heartwood_p2p::scoring::Violation;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};
use heartwood_p2p::{ChainView, NetworkMonitor, NodeServices, Peer, PeerSide};
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::CurrentRound;
use heartwood_p2p_wire::status::PeerConfig;
use tokio::sync::watch;

use crate::views::{Links, NodeView};

/// The node's services for the server.
pub struct RelayServices {
    /// The node thread.
    pub handle: NodeHandle,
    /// The node as the client reads it.
    pub view: Arc<NodeView>,
    /// The server's own read-only connections to the store.
    pub history: Arc<Readers>,
    /// The monitor and the machine.
    pub links: Arc<Links>,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
}

impl NodeServices for RelayServices {
    fn is_booted(&self) -> bool {
        todo!("the blockchain's start")
    }

    fn first_synced_ms(&self) -> Option<i64> {
        todo!("the first sync")
    }

    fn queue_state(&self) -> watch::Receiver<QueueState> {
        todo!("the queue's state")
    }

    fn chain(&self) -> Arc<dyn ChainView> {
        todo!("the chain view")
    }

    fn last_downloaded_block(&self) -> ChainLink {
        todo!("the last downloaded block")
    }

    fn ping_block(&self, header: &BlockHeader) -> bool {
        let _ = header;
        todo!("the block ping")
    }

    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr) {
        let _ = (block, from_forger, ip);
        todo!("an incoming block")
    }

    fn delegate_rank(&self, username: &str) -> Option<u32> {
        let _ = username;
        todo!("a delegate's rank")
    }

    fn already_forged(&self, height: u32, username: &str) -> Result<bool, LookupError> {
        let _ = (height, username);
        todo!("a delegate's block in the round")
    }

    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        todo!("the next round's delegates")
    }

    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        let _ = (from, limit, headers_only);
        todo!("the blocks for a download")
    }

    fn common_blocks(&self, ids: &[BlockId]) -> Result<Vec<BlockHeader>, LookupError> {
        let _ = ids;
        todo!("the common blocks")
    }

    fn pool_size(&self) -> usize {
        todo!("the pool's size")
    }

    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        let _ = (validate, exclude);
        todo!("the collation")
    }

    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        let _ = transactions;
        todo!("the P2P channel")
    }

    fn current_round(&self, now_ms: i64) -> Result<CurrentRound, LookupError> {
        let _ = now_ms;
        todo!("the current round")
    }

    fn slot_number(&self, timestamp: Option<i64>, now_ms: i64) -> i64 {
        let _ = (timestamp, now_ms);
        todo!("a slot number")
    }

    fn force_wakeup(&self) {
        todo!("the blockchain's wake-up")
    }

    fn forger_event(&self, event: ForgerEvent) {
        let _ = event;
        todo!("a forger's event")
    }

    fn peer_config(&self) -> PeerConfig {
        todo!("the peer configuration")
    }
}

/// The network monitor as the server's peer side.
pub struct MonitorSide<C, A, R>(pub Arc<NetworkMonitor<C, A, R>>);

impl<C, A, R> PeerSide for MonitorSide<C, A, R>
where
    C: PeerClient + 'static,
    A: PeerAcceptor + 'static,
    R: PeerRules + 'static,
{
    fn accept(&self, ip: IpAddr, port: u16) {
        let _ = (&self.0, ip, port);
        todo!("a caller accepted as a peer")
    }

    fn is_peer(&self, ip: IpAddr) -> bool {
        let _ = ip;
        todo!("a known peer")
    }

    fn record_version(&self, ip: IpAddr, version: &str) {
        let _ = (ip, version);
        todo!("a peer's version")
    }

    fn mark_stale(&self, ip: IpAddr) {
        let _ = ip;
        todo!("a stale peer")
    }

    fn record_violation(&self, ip: IpAddr, violation: Violation) {
        let _ = (ip, violation);
        todo!("a transport violation")
    }

    fn peers(&self) -> Vec<Peer> {
        todo!("the peers")
    }

    fn remove_keys(&self, keys: &[PublicKeyBytes]) {
        let _ = keys;
        todo!("the delegates on this node taken from the peers")
    }

    fn is_cold_start(&self) -> bool {
        todo!("the cold start")
    }

    fn complete_cold_start(&self) {
        todo!("the cold start's end")
    }

    fn cleanse(&self, options: CleanseOptions) -> BoxFuture<'_, ()> {
        let _ = options;
        todo!("a cleanse")
    }
}
