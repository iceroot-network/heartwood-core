//! Fakes of the server's neighbours, for the server's tests, its fuzz target and the harness's
//! subject of its plugin chain: a node ([`FakeNode`]) with a chain at a height, a boot flag, a
//! first sync and a queue state; a peer side ([`FakePeers`]) that records what the server tells
//! it; route handlers ([`StubHandlers`]) that answer after an optional wait; and a recorder of the
//! sockets' opens and closes ([`RecordedEvents`]).
//!
//! [`serve_request`] and [`inbound_rate_limit`] answer the oracle's `p2p.serveRequest` and the
//! inbound `ratelimit.run` with the server's code, for the crate's vector test and the harness.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::BoxFuture;
use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{
    Aux, Block, BlockHeader, BlockId, KeyPair, Milestones, Network, PublicKeyBytes, TransactionId,
};
use heartwood_kernel::ChainLink;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::BlockListing;
use heartwood_p2p_wire::internal::CurrentRound;
use heartwood_p2p_wire::nes::Message;
use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, TokenConfig};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::errors::LookupError;
use crate::monitor::CleanseOptions;
use crate::node_services::{
    Collation, ForgerEvent, NodeServices, NodeUnavailable, PeerSide, QueueState,
};
use crate::peer::Peer;
use crate::scoring::Violation;
use crate::server::ServerOptions;
use crate::server::ban_hammer::BanHammer;
use crate::server::plugins::{PluginParts, Plugins};
use crate::server::rate_limit::InboundLimiter;
use crate::server::seams::{RouteHandlers, ServerRefusal, ServerRequest, SessionEvents, SessionId};
use crate::server::session::{Reply, reply, route_of};
use crate::view::{BlockPing, ChainView, DelegateInfo};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A chain whose last block is at a height, under milestones and a network.
pub struct FakeChain {
    last: Arc<Block>,
    milestones: Milestones,
    network: Network,
}

impl FakeChain {
    /// A chain with its last block at `height`, signed by a test key.
    pub fn at(height: u32, milestones: Milestones, network: Network) -> Result<FakeChain, String> {
        let draft = BlockDraft {
            version: 0,
            timestamp: height.saturating_mul(8),
            height,
            previous_block: [1; 32],
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 0,
            payload_length: 0,
            payload_hash: [0; 32],
        };
        let keys = KeyPair::from_passphrase("server fake").map_err(|e| e.to_string())?;
        let block = factory::make(draft, Vec::new(), &keys, Aux::random(), &milestones)
            .map_err(|e| e.to_string())?;
        let last = Arc::new(block);
        Ok(FakeChain {
            last,
            milestones,
            network,
        })
    }
}

impl ChainView for FakeChain {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.last)
    }

    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![self.last_block()]
    }

    fn milestones(&self) -> &Milestones {
        &self.milestones
    }

    fn network(&self) -> &Network {
        &self.network
    }

    fn delegate_by_key(&self, _key: &PublicKeyBytes) -> Option<DelegateInfo> {
        None
    }

    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        Vec::new()
    }

    fn is_synced(&self) -> bool {
        true
    }

    fn is_idle(&self) -> bool {
        true
    }

    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

/// A node: its chain, whether it has booted, its first sync and its queue.
pub struct FakeNode {
    chain: Arc<FakeChain>,
    booted: AtomicBool,
    first_synced: Mutex<Option<i64>>,
    queue: watch::Sender<QueueState>,
}

impl FakeNode {
    /// A booted node over `chain`, never synced, its queue idle.
    pub fn new(chain: FakeChain) -> FakeNode {
        FakeNode {
            chain: Arc::new(chain),
            booted: AtomicBool::new(true),
            first_synced: Mutex::new(None),
            queue: watch::Sender::new(QueueState::default()),
        }
    }

    /// Set whether the node has booted.
    pub fn set_booted(&self, booted: bool) {
        self.booted.store(booted, Ordering::SeqCst);
    }

    /// Set when the node first synced.
    pub fn set_first_synced(&self, at_ms: Option<i64>) {
        *lock(&self.first_synced) = at_ms;
    }

    /// Set the queue's state.
    pub fn set_queue(&self, state: QueueState) {
        self.queue.send_replace(state);
    }
}

impl NodeServices for FakeNode {
    fn is_booted(&self) -> bool {
        self.booted.load(Ordering::SeqCst)
    }

    fn first_synced_ms(&self) -> Option<i64> {
        *lock(&self.first_synced)
    }

    fn queue_state(&self) -> watch::Receiver<QueueState> {
        self.queue.subscribe()
    }

    fn chain(&self) -> Arc<dyn ChainView> {
        Arc::clone(&self.chain) as Arc<dyn ChainView>
    }

    fn last_downloaded_block(&self) -> ChainLink {
        ChainLink::of(&self.chain.last)
    }

    fn ping_block(&self, _header: &BlockHeader) -> bool {
        false
    }

    fn handle_incoming_block(&self, _block: Block, _from_forger: bool, _ip: IpAddr) {}

    fn delegate_rank(&self, _username: &str) -> Option<u32> {
        None
    }

    fn already_forged(&self, _height: u32, _username: &str) -> Result<bool, LookupError> {
        Ok(false)
    }

    fn active_keys_next_round(&self) -> Result<Vec<PublicKeyBytes>, LookupError> {
        Ok(Vec::new())
    }

    fn blocks_for_download(
        &self,
        _from: u32,
        _limit: u32,
        _headers_only: bool,
    ) -> Result<Vec<BlockListing>, LookupError> {
        Ok(Vec::new())
    }

    fn common_blocks(&self, _: &[BlockId], _: usize) -> Result<Vec<BlockHeader>, LookupError> {
        Ok(Vec::new())
    }

    fn pool_size(&self) -> usize {
        0
    }

    fn collate(
        &self,
        _validate: bool,
        _exclude: Vec<TransactionId>,
    ) -> BoxFuture<'_, Result<Collation, NodeUnavailable>> {
        Box::pin(async { Ok(Collation::default()) })
    }

    fn post_transactions(
        &self,
        _transactions: Vec<Vec<u8>>,
    ) -> BoxFuture<'_, Result<Vec<TransactionId>, NodeUnavailable>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn current_round(&self, _now_ms: i64) -> Result<CurrentRound, LookupError> {
        Err(LookupError("no round in the fake node".to_owned()))
    }

    fn slot_number(&self, _timestamp: Option<i64>, _now_ms: i64) -> i64 {
        0
    }

    fn force_wakeup(&self) {}

    fn forger_event(&self, _event: ForgerEvent) {}

    fn peer_config(&self) -> PeerConfig {
        let network = &self.chain.network;
        PeerConfig {
            version: "4.3.1".to_owned(),
            network: NetworkConfig {
                version: network.pub_key_hash,
                name: network.name.clone(),
                nethash: String::new(),
                explorer: String::new(),
                token: TokenConfig {
                    name: String::new(),
                    symbol: String::new(),
                },
            },
            plugins: Vec::new(),
        }
    }
}

/// One thing the server told the peer side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerCall {
    /// `accept(ip, port)`.
    Accept(IpAddr, u16),
    /// `record_version(ip, version)`.
    Version(IpAddr, String),
    /// `mark_stale(ip)`.
    Stale(IpAddr),
    /// `record_violation(ip, violation)`.
    Violation(IpAddr, Violation),
}

/// A peer side with a set of known peers, recording what the server tells it.
#[derive(Debug, Default)]
pub struct FakePeers {
    known: Mutex<HashSet<IpAddr>>,
    calls: Mutex<Vec<PeerCall>>,
}

impl FakePeers {
    /// Know `ip` as a peer.
    pub fn know(&self, ip: IpAddr) {
        lock(&self.known).insert(ip);
    }

    /// What the server told the peer side, in order.
    pub fn calls(&self) -> Vec<PeerCall> {
        lock(&self.calls).clone()
    }

    /// The violations recorded, in order.
    pub fn violations(&self) -> Vec<(IpAddr, Violation)> {
        let calls = self.calls().into_iter();
        let violation = |call| match call {
            PeerCall::Violation(ip, violation) => Some((ip, violation)),
            _ => None,
        };
        calls.filter_map(violation).collect()
    }

    fn record(&self, call: PeerCall) {
        lock(&self.calls).push(call);
    }
}

impl PeerSide for FakePeers {
    fn accept(&self, ip: IpAddr, port: u16) {
        self.record(PeerCall::Accept(ip, port));
    }

    fn is_peer(&self, ip: IpAddr) -> bool {
        lock(&self.known).contains(&ip)
    }

    fn record_version(&self, ip: IpAddr, version: &str) {
        if self.is_peer(ip) {
            self.record(PeerCall::Version(ip, version.to_owned()));
        }
    }

    fn mark_stale(&self, ip: IpAddr) {
        self.record(PeerCall::Stale(ip));
    }

    fn record_violation(&self, ip: IpAddr, violation: Violation) {
        self.record(PeerCall::Violation(ip, violation));
    }

    fn peers(&self) -> Vec<Peer> {
        Vec::new()
    }

    fn remove_keys(&self, _keys: &[PublicKeyBytes]) {}

    fn is_cold_start(&self) -> bool {
        false
    }

    fn complete_cold_start(&self) {}

    fn cleanse(&self, _options: CleanseOptions) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// How a stub handler answers a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StubAnswer {
    /// The payload.
    Payload(Vec<u8>),
    /// A refusal.
    Refuse(ServerRefusal),
    /// The payload after a wait.
    After(Duration, Vec<u8>),
}

/// Route handlers that answer each route as scripted (an empty payload by default), recording
/// every request they get.
#[derive(Debug, Default)]
pub struct StubHandlers {
    answers: Mutex<Vec<(RouteId, StubAnswer)>>,
    requests: Mutex<Vec<(RouteId, ServerRequest)>>,
}

impl StubHandlers {
    /// Answer `route` with `answer` from now on.
    pub fn answer(&self, route: RouteId, answer: StubAnswer) {
        let mut answers = lock(&self.answers);
        answers.retain(|(known, _)| *known != route);
        answers.push((route, answer));
    }

    /// The requests the handlers got, in order.
    pub fn requests(&self) -> Vec<(RouteId, ServerRequest)> {
        lock(&self.requests).clone()
    }
}

impl RouteHandlers for StubHandlers {
    fn handle(
        &self,
        route: RouteId,
        request: ServerRequest,
    ) -> BoxFuture<'_, Result<Vec<u8>, ServerRefusal>> {
        lock(&self.requests).push((route, request));
        let answer = lock(&self.answers)
            .iter()
            .find(|(known, _)| *known == route)
            .cloned();
        Box::pin(async move {
            match answer.map(|(_, answer)| answer) {
                None => Ok(Vec::new()),
                Some(StubAnswer::Payload(payload)) => Ok(payload),
                Some(StubAnswer::Refuse(refusal)) => Err(refusal),
                Some(StubAnswer::After(wait, payload)) => {
                    tokio::time::sleep(wait).await;
                    Ok(payload)
                }
            }
        })
    }
}

/// A recorder of the sockets' opens (`true`) and closes (`false`).
#[derive(Debug, Default)]
pub struct RecordedEvents {
    events: Mutex<Vec<(SessionId, IpAddr, bool)>>,
}

impl RecordedEvents {
    /// The opens and closes, in order.
    pub fn events(&self) -> Vec<(SessionId, IpAddr, bool)> {
        lock(&self.events).clone()
    }
}

impl SessionEvents for RecordedEvents {
    fn opened(&self, session: SessionId, ip: IpAddr) {
        lock(&self.events).push((session, ip, true));
    }

    fn closed(&self, session: SessionId, ip: IpAddr) {
        lock(&self.events).push((session, ip, false));
    }
}

/// The reference's default `minimumVersions` option.
const DEFAULT_RANGES: [&str; 2] = ["^3.2", "^3.2.0-next.0"];

/// The oracle's `p2p.serveRequest` answered with the server: `input` is `{route, hex, ip,
/// remoteAccess, booted?, known?, minimumVersions?, maxTransactionsPerRequest?}`, the node's tip
/// at `height` under `milestones`. The output is the oracle's `{status, message, sent, terminated,
/// banReason, connectionClose, acceptPort, versionRecorded}`, a codec's refusal cut after its
/// error's class as the oracle cuts it.
pub async fn serve_request(
    input: &Value,
    height: u32,
    milestones: Milestones,
    network: Network,
) -> Result<Value, String> {
    let text = |key: &str| {
        input[key]
            .as_str()
            .ok_or(format!("input.{key} is not text"))
    };
    let texts = |value: &Value| -> Vec<String> {
        let items = value.as_array().into_iter().flatten();
        items
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    };
    let ip: IpAddr = text("ip")?
        .parse()
        .map_err(|_| "input.ip is not an address")?;
    let ip = ip.to_canonical();
    let payload = hex::decode(text("hex")?).map_err(|e| e.to_string())?;
    let remote_access: Vec<IpAddr> = (texts(&input["remoteAccess"]).iter())
        .filter_map(|entry| entry.parse::<IpAddr>().ok().map(|ip| ip.to_canonical()))
        .collect();
    let mut minimum_versions = texts(&input["minimumVersions"]);
    if input.get("minimumVersions").is_none_or(Value::is_null) {
        minimum_versions = DEFAULT_RANGES.map(str::to_owned).to_vec();
    }
    let max_transactions = input["maxTransactionsPerRequest"].as_u64().unwrap_or(40);
    let options = ServerOptions {
        host: IpAddr::from([127, 0, 0, 1]),
        port: 4002,
        ban_seconds: 10,
        remote_access: remote_access.clone(),
        minimum_versions,
        rate_limit: 100,
        rate_limit_post_transactions: 25,
        max_transactions_per_request: usize::try_from(max_transactions).unwrap_or(usize::MAX),
        max_same_subnet_peers: 256,
        await_block_bound: Duration::from_secs(8),
        attestation_wait: Duration::from_millis(500),
        addressing: crate::devnet::Addressing::Off,
    };
    let node = Arc::new(FakeNode::new(FakeChain::at(height, milestones, network)?));
    node.set_booted(input["booted"].as_bool() != Some(false));
    let peers = Arc::new(FakePeers::default());
    if input["known"].as_bool() == Some(true) {
        peers.know(ip);
    }
    let bans = Arc::new(BanHammer::new(10, remote_access));
    let plugins = Plugins::new(PluginParts {
        options: Arc::new(options),
        node,
        peers: Arc::clone(&peers) as Arc<dyn PeerSide>,
        handlers: Arc::new(StubHandlers::default()),
        ban_hammer: Arc::clone(&bans),
        clock: Arc::new(heartwood_kernel::SystemClock),
    });
    // The path as the socket's parser reads it.
    let route = text("route")?.to_owned();
    let framed = Message {
        path: route,
        ..Message::request(2, RouteId::GetPeers, Vec::new())
    };
    let path = (framed
        .encode()
        .ok()
        .and_then(|bytes| Message::parse(&bytes).ok()))
    .map(|parsed| parsed.path)
    .ok_or("the route does not fit a frame")?;
    let (status, message, close, reply) = match route_of(&path) {
        Err(reason) => {
            let status = if reason == "Not Found" { 404 } else { 400 };
            let reply = Reply::Terminate(reason.to_owned());
            (status, Some(reason.to_owned()), false, reply)
        }
        Ok(route) => {
            let answer = plugins.handle(route, payload, ip, SessionId(1)).await;
            let (status, message) = match &answer.result {
                Ok(_) => (200, None),
                Err(refusal) => (refusal.status, Some(refusal.message.clone())),
            };
            (status, message, answer.close, reply(answer))
        }
    };
    let (sent, terminated) = match reply {
        Reply::Payload(_) => (None, None),
        Reply::Graceful(body) => (Some(String::from_utf8_lossy(&body).into_owned()), None),
        Reply::Terminate(reason) => (None, Some(reason)),
    };
    // The ban hammer's close handler: the socket's reason bans an address not yet banned.
    if let Some(reason) = &terminated
        && bans.reason(ip).is_none()
    {
        bans.ban(ip, reason, 0);
    }
    let calls = peers.calls();
    let accept_port = calls.iter().find_map(|call| match call {
        PeerCall::Accept(_, port) => Some(*port),
        _ => None,
    });
    let version = calls.iter().find_map(|call| match call {
        PeerCall::Version(_, version) => Some(version.clone()),
        _ => None,
    });
    Ok(json!({
        "status": status,
        "message": message.map(cut),
        "sent": sent,
        "terminated": terminated.map(cut),
        "banReason": bans.reason(ip).map(cut),
        "connectionClose": close,
        "acceptPort": accept_port,
        "versionRecorded": version,
    }))
}

/// A codec's refusal cut after its error's class, as the oracle cuts the reference's.
fn cut(text: String) -> String {
    const FAILED: &str = "Payload deserialising failed: ";
    let class = text
        .strip_prefix(FAILED)
        .and_then(|rest| rest.split_once(": "));
    class.map_or(text.clone(), |(class, _)| format!("{FAILED}{class}"))
}

/// The oracle's `ratelimit.run` with `limiter.inbound`, answered with the server's limiter: each
/// event `{t, ip, route, op: "consume"}` at `t` milliseconds is `ok` or `exceeded`, the addresses
/// of `limiter.remoteAccess` always `ok`.
pub fn inbound_rate_limit(input: &Value) -> Result<Value, String> {
    let limiter = &input["limiter"];
    let points = |key: &str| {
        (limiter[key].as_u64().and_then(|n| u32::try_from(n).ok()))
            .ok_or(format!("input.limiter.{key} is not a count"))
    };
    let exempt: Vec<IpAddr> = (limiter["remoteAccess"].as_array().into_iter().flatten())
        .filter_map(|entry| entry.as_str()?.parse().ok())
        .collect();
    let inbound = InboundLimiter::new(points("rateLimit")?, points("rateLimitPostTransactions")?);
    let start = Instant::now();
    let mut answers = Vec::new();
    for event in input["events"]
        .as_array()
        .ok_or("input.events is not a list")?
    {
        let at = event["t"].as_u64().ok_or("an event's t is not a count")?;
        let ip: IpAddr = (event["ip"].as_str().and_then(|ip| ip.parse().ok()))
            .ok_or("an event's ip is not an address")?;
        let name = event["route"].as_str().unwrap_or_default();
        let route = (RouteId::ALL.into_iter().find(|route| route.name() == name))
            .ok_or(format!("no route named {name}"))?;
        if event["op"] != "consume" {
            return Err("the server's limiter only consumes".to_owned());
        }
        let now = start
            .checked_add(Duration::from_millis(at))
            .unwrap_or(start);
        let ok = exempt.contains(&ip) || inbound.consume(ip, route, now);
        answers.push(if ok { "ok" } else { "exceeded" });
    }
    Ok(json!({ "answers": answers }))
}
