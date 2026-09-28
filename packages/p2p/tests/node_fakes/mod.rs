//! Fakes of the server's neighbours for the tests of the controllers, the attestation session and
//! the network state: the node's services over the peer fakes' chain, the peer side, a wall clock
//! that follows tokio's (paused) clock, and the controllers made of them. Each test file uses part
//! of them.

#![allow(dead_code)]

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use heartwood_crypto::{Block, BlockHeader, BlockId, PublicKeyBytes, TransactionId};
use heartwood_kernel::{ChainLink, Clock};
use heartwood_p2p::attestation_session::AttestationSession;
use heartwood_p2p::errors::LookupError;
use heartwood_p2p::monitor::CleanseOptions;
use heartwood_p2p::node_services::{Collation, ForgerEvent, NodeUnavailable, QueueState};
use heartwood_p2p::scoring::Violation;
use heartwood_p2p::server::controllers::{ControllerParts, Controllers};
use heartwood_p2p::server::seams::{ServerRequest, SessionId};
use heartwood_p2p::status_server::StatusServer;
use heartwood_p2p::view::ChainView;
use heartwood_p2p::{NodeServices, P2pOptions, Peer, PeerSide};
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::{CurrentRound, LastBlockData, NetworkState};
use heartwood_p2p_wire::status::PeerConfig;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::peer_fakes::{Chain, DEVNET, peer_config};

/// A wall clock that starts at `base_ms` and moves with tokio's clock, so that a paused runtime
/// moves both.
pub struct TokioClock {
    base_ms: i64,
    start: Instant,
}

impl TokioClock {
    pub fn at(base_ms: i64) -> TokioClock {
        TokioClock {
            base_ms,
            start: Instant::now(),
        }
    }
}

impl Clock for TokioClock {
    fn now_ms(&self) -> i64 {
        self.base_ms + self.start.elapsed().as_millis() as i64
    }
}

/// The node's services, answering from what the test put in and recording what it was asked.
pub struct Node {
    pub chain: Arc<Chain>,
    pub queue: watch::Sender<QueueState>,
    pub config: PeerConfig,
    pub last_downloaded: Mutex<ChainLink>,
    /// The id `ping_block` knows as the pinged block.
    pub pinged: Mutex<Option<BlockId>>,
    pub incoming: Mutex<Vec<(Block, bool, IpAddr)>>,
    pub already_forged: AtomicBool,
    pub active: Mutex<Vec<PublicKeyBytes>>,
    pub stored: Mutex<Vec<BlockListing>>,
    pub downloads: Mutex<Vec<(u32, u32, bool)>>,
    pub common: Mutex<Vec<BlockHeader>>,
    pub asked_common: Mutex<Vec<Vec<BlockId>>>,
    pub pool: Mutex<Vec<Vec<u8>>>,
    pub collations: Mutex<Vec<(bool, Vec<TransactionId>)>>,
    pub posted: Mutex<Vec<Vec<Vec<u8>>>>,
    pub accepted: Mutex<Vec<TransactionId>>,
    pub round: Mutex<Option<CurrentRound>>,
    pub round_times: Mutex<Vec<i64>>,
    pub wakeups: AtomicUsize,
    pub events: Mutex<Vec<ForgerEvent>>,
    pub served: Mutex<Vec<(IpAddr, NetworkState)>>,
}

impl Node {
    pub fn over(chain: Chain) -> Node {
        let tip = ChainLink::of(&chain.last_block());
        Node {
            chain: Arc::new(chain),
            queue: watch::channel(QueueState::default()).0,
            config: peer_config("4.3.1", &DEVNET.network.nethash),
            last_downloaded: Mutex::new(tip),
            pinged: Mutex::new(None),
            incoming: Mutex::default(),
            already_forged: AtomicBool::new(false),
            active: Mutex::default(),
            stored: Mutex::default(),
            downloads: Mutex::default(),
            common: Mutex::default(),
            asked_common: Mutex::default(),
            pool: Mutex::default(),
            collations: Mutex::default(),
            posted: Mutex::default(),
            accepted: Mutex::default(),
            round: Mutex::new(None),
            round_times: Mutex::default(),
            wakeups: AtomicUsize::new(0),
            events: Mutex::default(),
            served: Mutex::default(),
        }
    }

    pub fn at(height: u32) -> Node {
        Node::over(Chain::at(height))
    }

    /// Make `block` the tip.
    pub fn tip(&self, block: Block) {
        *self.chain.last.lock().unwrap() = Arc::new(block);
    }
}

impl NodeServices for Node {
    fn is_booted(&self) -> bool {
        true
    }

    fn first_synced_ms(&self) -> Option<i64> {
        None
    }

    fn queue_state(&self) -> watch::Receiver<QueueState> {
        self.queue.subscribe()
    }

    fn chain(&self) -> Arc<dyn ChainView> {
        self.chain.clone()
    }

    fn last_downloaded_block(&self) -> ChainLink {
        *self.last_downloaded.lock().unwrap()
    }

    fn ping_block(&self, header: &BlockHeader) -> bool {
        let id = heartwood_crypto::blocks::serialiser::block_id(header);
        *self.pinged.lock().unwrap() == Some(id)
    }

    fn handle_incoming_block(&self, block: Block, from_forger: bool, ip: IpAddr) {
        self.incoming.lock().unwrap().push((block, from_forger, ip));
    }

    fn delegate_rank(&self, username: &str) -> Option<u32> {
        let keys = self.chain.delegate_keys();
        let known = keys
            .iter()
            .filter_map(|key| self.chain.delegate_by_key(key));
        known
            .into_iter()
            .find(|delegate| delegate.username == username)?
            .rank
    }

    fn already_forged(&self, _height: u32, _username: &str) -> Result<bool, LookupError> {
        Ok(self.already_forged.load(Ordering::SeqCst))
    }

    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        Ok(self.active.lock().unwrap().clone())
    }

    fn blocks_for_download(
        &self,
        from: u32,
        limit: u32,
        headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        self.downloads
            .lock()
            .unwrap()
            .push((from, limit, headers_only));
        let stored = self.stored.lock().unwrap();
        let wanted = stored
            .iter()
            .filter(|block| block.height >= from && block.height < from + limit);
        Ok(wanted.cloned().collect())
    }

    fn common_blocks(&self, ids: &[BlockId]) -> Result<Vec<BlockHeader>, LookupError> {
        self.asked_common.lock().unwrap().push(ids.to_vec());
        let common = self.common.lock().unwrap();
        let known = |header: &&BlockHeader| {
            ids.contains(&heartwood_crypto::blocks::serialiser::block_id(header))
        };
        Ok(common.iter().filter(known).cloned().collect())
    }

    fn pool_size(&self) -> usize {
        self.pool.lock().unwrap().len() + 7
    }

    fn collate(
        &self,
        validate: bool,
        exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        Box::pin(async move {
            self.collations.lock().unwrap().push((validate, exclude));
            let transactions = self.pool.lock().unwrap().clone();
            Ok(Collation {
                pool_size: transactions.len(),
                transactions,
            })
        })
    }

    fn post_transactions(
        &self,
        transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        Box::pin(async move {
            self.posted.lock().unwrap().push(transactions);
            Ok(self.accepted.lock().unwrap().clone())
        })
    }

    fn current_round(&self, now_ms: i64) -> Result<CurrentRound, LookupError> {
        self.round_times.lock().unwrap().push(now_ms);
        let round = self.round.lock().unwrap().clone();
        round.ok_or_else(|| LookupError("no round".into()))
    }

    fn slot_number(&self, timestamp: Option<i64>, now_ms: i64) -> i64 {
        let seconds = timestamp.unwrap_or((now_ms - crate::peer_fakes::EPOCH_MS) / 1_000);
        seconds.div_euclid(8)
    }

    fn force_wakeup(&self) {
        self.wakeups.fetch_add(1, Ordering::SeqCst);
    }

    fn forger_event(&self, event: ForgerEvent) {
        self.events.lock().unwrap().push(event);
    }

    fn peer_config(&self) -> PeerConfig {
        self.config.clone()
    }

    fn network_state_served(&self, caller: IpAddr, state: &NetworkState) {
        self.served.lock().unwrap().push((caller, state.clone()));
    }
}

/// The peer side: the peers the test put in, and what the server asked of it.
#[derive(Default)]
pub struct Peers {
    pub peers: Mutex<Vec<Peer>>,
    pub removed: Mutex<Vec<Vec<PublicKeyBytes>>>,
    pub cleanses: Mutex<Vec<CleanseOptions>>,
    pub cold: AtomicBool,
    pub cold_ended: AtomicBool,
}

impl PeerSide for Peers {
    fn accept(&self, _ip: IpAddr, _port: u16) {}

    fn is_peer(&self, ip: IpAddr) -> bool {
        self.peers.lock().unwrap().iter().any(|peer| peer.ip == ip)
    }

    fn record_version(&self, _ip: IpAddr, _version: &str) {}

    fn mark_stale(&self, _ip: IpAddr) {}

    fn record_violation(&self, _ip: IpAddr, _violation: Violation) {}

    fn peers(&self) -> Vec<Peer> {
        self.peers.lock().unwrap().clone()
    }

    fn remove_keys(&self, keys: &[PublicKeyBytes]) {
        self.removed.lock().unwrap().push(keys.to_vec());
        for peer in self.peers.lock().unwrap().iter_mut() {
            peer.public_keys.retain(|key| !keys.contains(key));
        }
    }

    fn is_cold_start(&self) -> bool {
        self.cold.load(Ordering::SeqCst) && !self.cold_ended.load(Ordering::SeqCst)
    }

    fn complete_cold_start(&self) {
        self.cold_ended.store(true, Ordering::SeqCst);
    }

    fn cleanse(&self, options: CleanseOptions) -> BoxFuture<'_, ()> {
        self.cleanses.lock().unwrap().push(options);
        Box::pin(async {})
    }
}

/// The server's side of a node: its services, its peers, the attestation session, the served
/// status and the controllers over them.
pub struct Server {
    pub node: Arc<Node>,
    pub peers: Arc<Peers>,
    pub session: Arc<AttestationSession>,
    pub status: Arc<StatusServer>,
    pub controllers: Controllers,
    pub clock: Arc<dyn Clock>,
}

impl Server {
    pub fn new(node: Node, clock: Arc<dyn Clock>) -> Server {
        Server::with(node, clock, P2pOptions::default())
    }

    pub fn with(node: Node, clock: Arc<dyn Clock>, options: P2pOptions) -> Server {
        let node = Arc::new(node);
        let peers = Arc::new(Peers::default());
        let session = Arc::new(AttestationSession::new(node.clone(), clock.clone()));
        let status = Arc::new(StatusServer::new(
            node.clone(),
            session.clone(),
            clock.clone(),
        ));
        let controllers = Controllers::new(ControllerParts {
            node: node.clone(),
            peers: peers.clone(),
            attestation: session.clone(),
            status: status.clone(),
            clock: clock.clone(),
            options: Arc::new(options),
        });
        Server {
            node,
            peers,
            session,
            status,
            controllers,
            clock,
        }
    }
}

/// The forger's address.
pub fn forger() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}

/// A request with `payload` from `ip` on session 1.
pub fn request(payload: Vec<u8>, ip: IpAddr, from_forger: bool) -> ServerRequest {
    ServerRequest {
        payload,
        ip,
        session: SessionId(1),
        from_forger,
    }
}

/// A round with no delegate and the tip at `height`.
pub fn round(height: u32) -> CurrentRound {
    let fields = serde_json::json!({"height": height, "id": "ab"});
    CurrentRound {
        all_delegates: Vec::new(),
        can_forge: true,
        current: 1,
        current_forger: None,
        delegates: Vec::new(),
        last_block: LastBlockData::new(fields.as_object().unwrap().clone()).unwrap(),
        next_forger: None,
        reward: "0".into(),
        timestamp: 80,
    }
}
