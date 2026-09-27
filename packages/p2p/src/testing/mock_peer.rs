//! The mock peer: a tokio listener on one IP that answers the client's requests from a script,
//! or serves a chain.
//!
//! The connector keeps one session per IP, so tests that need several peers bind each mock peer
//! to its own loopback address (127.0.0.1, 127.0.0.2, ...). The peer speaks nes through the wire
//! crate on the server side of tungstenite: it answers the hello with its heartbeat, sends its
//! pings, and answers each request.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use heartwood_crypto::blocks::factory;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, KeyPair, Milestones, Network};
use heartwood_kernel::{Clock, SystemClock};
use heartwood_p2p_wire::codecs::blocks::{self as block_codec, ListedBlock};
use heartwood_p2p_wire::codecs::{peer as peer_codec, transactions as tx_codec};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::{BlockHeader, PostBlockResponse};
use heartwood_p2p_wire::messages::peer::{Common, GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use heartwood_p2p_wire::replies::PeerAddress;
use heartwood_p2p_wire::status::{self, PeerConfig, SignedState, StatusHeaderData};
use heartwood_p2p_wire::{Amount, RouteId};
use indexmap::IndexMap;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::transport::{Dial, Stream};

/// A frame the mock peer sends instead of an answer, for the protocol tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockFrame {
    /// A binary frame with these bytes (a nes message, or not).
    Binary(Vec<u8>),
    /// A text frame.
    Text(String),
    /// A WebSocket ping frame.
    Ping,
    /// A WebSocket pong frame.
    Pong,
}

/// What the mock peer does with one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A response with status 200 and this payload.
    Payload(Vec<u8>),
    /// A nes error with this status and payload.
    Error {
        /// The status code.
        status: u16,
        /// The payload.
        payload: Vec<u8>,
    },
    /// Wait, then do the inner reply; later requests are answered meanwhile.
    After(Duration, Box<Reply>),
    /// No answer at all.
    Silence,
    /// Close the connection.
    Close,
    /// Send these frames instead of an answer.
    Frames(Vec<MockFrame>),
}

/// The replies of a scripted mock peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The heartbeat of the hello reply; the reference's 15 s and 5 s by default.
    pub heartbeat: Heartbeat,
    /// Server pings sent after the hello, each after its delay from the previous one.
    pub pings: Vec<Duration>,
    /// Per route, the replies to its requests in order; a route with no reply left gets
    /// [`Reply::Silence`].
    pub replies: IndexMap<RouteId, VecDeque<Reply>>,
}

impl Default for Script {
    fn default() -> Script {
        Script {
            heartbeat: Heartbeat {
                interval: 15_000,
                timeout: 5_000,
            },
            pings: Vec::new(),
            replies: IndexMap::new(),
        }
    }
}

impl Script {
    /// A script that answers nothing.
    pub fn new() -> Script {
        Script::default()
    }

    /// The script with `reply` added after the replies already set for `route`.
    pub fn on(mut self, route: RouteId, reply: Reply) -> Script {
        self.replies.entry(route).or_default().push_back(reply);
        self
    }
}

/// A chain the mock peer serves: getStatus from its last block, getBlocks and getCommonBlocks from
/// its blocks, getPeers from `peers`; postBlock, postTransactions and getUnconfirmedTransactions
/// are accepted with empty answers. It pings every 15 s, as the reference's server does.
pub struct ServedChain {
    /// The blocks from height 1, in height order.
    pub blocks: Vec<Block>,
    /// The chain's milestones.
    pub milestones: Milestones,
    /// The configuration the status reports.
    pub config: PeerConfig,
    /// The keys whose attestations the status carries.
    pub signers: Vec<KeyPair>,
    /// The peers getPeers lists.
    pub peers: Vec<PeerAddress>,
    /// Whether the first getBlocks is answered with no block.
    pub empty_first_get_blocks: bool,
}

/// The mock peer could not start.
#[derive(Debug, Error)]
pub enum MockPeerError {
    /// The listener could not be bound.
    #[error("the mock peer cannot listen: {0}")]
    Listen(#[from] std::io::Error),
    /// The standalone process's arguments or chain directory are not usable.
    #[error("{0}")]
    Usage(String),
}

/// A running mock peer. Dropping it stops it.
pub struct MockPeer {
    address: SocketAddr,
    behaviour: Arc<Behaviour>,
    log: Arc<Mutex<Log>>,
}

#[derive(Debug, Default)]
struct Log {
    requests: Vec<(RouteId, Vec<u8>)>,
    connections: usize,
    /// The listener's and the connections' tasks, stopped with the peer.
    tasks: Vec<AbortHandle>,
}

impl std::fmt::Debug for MockPeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockPeer")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

enum Answers {
    Script(Mutex<IndexMap<RouteId, VecDeque<Reply>>>),
    Chain(Box<ServedChain>, AtomicBool),
}

struct Behaviour {
    heartbeat: Heartbeat,
    /// The pings' delays; the last one repeats when `repeat` is set.
    pings: Vec<Duration>,
    repeat: bool,
    answers: Answers,
}

type Outbox = mpsc::UnboundedSender<Option<Frame>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MockPeer {
    /// Start a peer on `ip`, on a free port, that answers from `script`.
    pub async fn scripted(ip: IpAddr, script: Script) -> Result<MockPeer, MockPeerError> {
        let behaviour = Behaviour {
            heartbeat: script.heartbeat,
            pings: script.pings,
            repeat: false,
            answers: Answers::Script(Mutex::new(script.replies)),
        };
        MockPeer::start((ip, 0).into(), behaviour).await
    }

    /// Start a peer on `ip`, on a free port, that serves `chain`.
    pub async fn serving(ip: IpAddr, chain: ServedChain) -> Result<MockPeer, MockPeerError> {
        MockPeer::start((ip, 0).into(), Behaviour::serving(chain)).await
    }

    async fn start(address: SocketAddr, behaviour: Behaviour) -> Result<MockPeer, MockPeerError> {
        let listener = TcpListener::bind(address).await?;
        let peer = MockPeer {
            address: listener.local_addr()?,
            behaviour: Arc::new(behaviour),
            log: Arc::default(),
        };
        let (behaviour, log) = (peer.behaviour.clone(), peer.log.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accept(stream, &behaviour, &log);
            }
        });
        lock(&peer.log).tasks.push(task.abort_handle());
        Ok(peer)
    }

    /// Connections to `peers` in memory, by IP, for a session or connector on tokio's paused
    /// clock, which cannot drive real sockets; an address of no peer is refused.
    pub fn dial(peers: &[&MockPeer]) -> Dial {
        let peers: Vec<_> = (peers.iter())
            .map(|peer| (peer.address.ip(), peer.behaviour.clone(), peer.log.clone()))
            .collect();
        Arc::new(move |address| {
            let peer = peers.iter().find(|(ip, _, _)| *ip == address.ip());
            let stream = peer.map(|(_, behaviour, log)| {
                let (client, server) = tokio::io::duplex(64 * 1024);
                accept(server, behaviour, log);
                Box::new(client) as Box<dyn Stream>
            });
            Box::pin(async move { stream.ok_or_else(|| io::ErrorKind::ConnectionRefused.into()) })
        })
    }

    /// The address the peer listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The requests received so far, in order: the route and the payload.
    pub fn requests(&self) -> Vec<(RouteId, Vec<u8>)> {
        lock(&self.log).requests.clone()
    }

    /// How many connections the peer has accepted.
    pub fn connections(&self) -> usize {
        lock(&self.log).connections
    }
}

impl Drop for MockPeer {
    fn drop(&mut self) {
        lock(&self.log).tasks.iter().for_each(AbortHandle::abort);
    }
}

/// Count and serve a new connection.
fn accept<S>(stream: S, behaviour: &Arc<Behaviour>, log: &Arc<Mutex<Log>>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let task = tokio::spawn(serve(stream, behaviour.clone(), log.clone()));
    let mut log = lock(log);
    log.connections = log.connections.saturating_add(1);
    log.tasks.push(task.abort_handle());
}

/// One connection: the WebSocket handshake, then each message in turn.
async fn serve<S>(stream: S, behaviour: Arc<Behaviour>, log: Arc<Mutex<Log>>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (mut sink, mut incoming) = socket.split();
    let (outbox, mut queue) = mpsc::unbounded_channel::<Option<Frame>>();
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        while let Some(Some(frame)) = queue.recv().await {
            if sink.send(frame).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });
    while let Some(Ok(Frame::Binary(bytes))) = incoming.next().await {
        let Ok(message) = Message::parse(&bytes) else {
            continue;
        };
        match message.kind {
            Kind::Hello => {
                let hello = Message::server_hello(message.id, behaviour.heartbeat, "mock".into());
                send(&outbox, &hello);
                tasks.spawn(pings(behaviour.clone(), outbox.clone()));
            }
            Kind::Request => {
                let Some(route) = RouteId::from_id(message.path.trim_start_matches('/')) else {
                    continue;
                };
                lock(&log).requests.push((route, message.payload.clone()));
                perform(
                    behaviour.answer(route, &message.payload),
                    message.id,
                    &outbox,
                );
            }
            Kind::Ping | Kind::Error => {}
        }
    }
}

async fn pings(behaviour: Arc<Behaviour>, outbox: Outbox) {
    let mut delays = behaviour.pings.iter().copied();
    let mut last = None;
    while let Some(delay) = delays.next().or(last.filter(|_| behaviour.repeat)) {
        last = Some(delay);
        tokio::time::sleep(delay).await;
        send(&outbox, &Message::ping(0));
    }
}

fn send(outbox: &Outbox, message: &Message) {
    if let Ok(bytes) = message.encode() {
        let _ = outbox.send(Some(Frame::Binary(bytes.into())));
    }
}

fn perform(reply: Reply, id: u32, outbox: &Outbox) {
    match reply {
        Reply::Payload(payload) => send(outbox, &Message::response(id, payload)),
        Reply::Error { status, payload } => send(
            outbox,
            &Message {
                status_code: status,
                ..Message::response(id, payload)
            },
        ),
        Reply::After(delay, reply) => {
            let outbox = outbox.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                perform(*reply, id, &outbox);
            });
        }
        Reply::Silence => {}
        Reply::Close => {
            let _ = outbox.send(None);
        }
        Reply::Frames(frames) => {
            for frame in frames {
                let frame = match frame {
                    MockFrame::Binary(bytes) => Frame::Binary(bytes.into()),
                    MockFrame::Text(text) => Frame::Text(text.into()),
                    MockFrame::Ping => Frame::Ping(Vec::new().into()),
                    MockFrame::Pong => Frame::Pong(Vec::new().into()),
                };
                let _ = outbox.send(Some(frame));
            }
        }
    }
}

impl Behaviour {
    fn serving(chain: ServedChain) -> Behaviour {
        let empty_first = AtomicBool::new(chain.empty_first_get_blocks);
        Behaviour {
            heartbeat: Script::default().heartbeat,
            pings: vec![Duration::from_secs(15)],
            repeat: true,
            answers: Answers::Chain(Box::new(chain), empty_first),
        }
    }

    fn answer(&self, route: RouteId, payload: &[u8]) -> Reply {
        let (chain, empty_first) = match &self.answers {
            Answers::Script(replies) => {
                let reply = lock(replies).get_mut(&route).and_then(VecDeque::pop_front);
                return reply.unwrap_or(Reply::Silence);
            }
            Answers::Chain(chain, empty_first) => (chain, empty_first),
        };
        let Some(tip) = chain.blocks.last() else {
            return Reply::Silence;
        };
        Reply::Payload(match route {
            RouteId::GetStatus => chain.status(tip),
            RouteId::GetPeers => {
                let peers: Vec<ListedPeer> = (chain.peers.iter())
                    .map(|peer| ListedPeer {
                        ip: Some(peer.ip.clone()),
                        port: Some(peer.port.into()),
                        ..ListedPeer::default()
                    })
                    .collect();
                peer_codec::get_peers_encode_response(&peers)
            }
            RouteId::GetCommonBlocks => {
                let ids = peer_codec::get_common_blocks_decode_request(payload)
                    .map(|request| request.ids)
                    .unwrap_or_default();
                let common = (chain.blocks.iter().rev())
                    .find(|block| ids.contains(&block.id().to_hex()))
                    .map(|block| Common {
                        height: Some(block.height()),
                        id: Some(block.id().to_hex()),
                        ..Common::default()
                    });
                peer_codec::get_common_blocks_encode_response(&GetCommonBlocksResponse {
                    common,
                    ..GetCommonBlocksResponse::default()
                })
            }
            RouteId::GetBlocks => {
                let request = block_codec::get_blocks_decode_request(payload).unwrap_or_default();
                let from = request.last_block_height.unwrap_or(0).saturating_add(1);
                let limit = request
                    .block_limit
                    .filter(|limit| *limit > 0)
                    .unwrap_or(400);
                let listed: Vec<ListedBlock> = if empty_first.swap(false, Ordering::Relaxed) {
                    Vec::new()
                } else {
                    (chain.blocks.iter())
                        .filter(|block| block.height() >= from)
                        .take(usize::try_from(limit).unwrap_or(usize::MAX))
                        .map(|block| listed(block, request.headers_only == Some(true)))
                        .collect()
                };
                block_codec::get_blocks_encode_response(&listed)
            }
            RouteId::PostBlock => block_codec::post_block_encode_response(&PostBlockResponse {
                status: Some(true),
                height: Some(tip.height()),
                ..PostBlockResponse::default()
            }),
            RouteId::GetUnconfirmedTransactions => {
                tx_codec::get_unconfirmed_transactions_encode_response(&Default::default())
            }
            RouteId::PostTransactions => tx_codec::post_transactions_encode_response(&[]),
            _ => return Reply::Silence,
        })
    }
}

impl ServedChain {
    /// The signed status of `tip` at the current slot.
    fn status(&self, tip: &Block) -> Vec<u8> {
        let params = self.milestones.at(tip.height());
        let epoch = slots::epoch_unix_ms(params.epoch()).unwrap_or(0);
        let now = slots::epoch_time(SystemClock.now_ms(), epoch);
        let slot = slots::slot_info(now, params.block_time());
        let state = SignedState {
            height: tip.height(),
            forging_allowed: slot.forging_status,
            current_slot: slot.slot_number,
            header: StatusHeaderData::from_block(tip),
        };
        let preimage = status::signing_preimage(&state, &self.config);
        let attestations: Vec<_> = (self.signers.iter())
            .filter_map(|key| status::attest(&preimage, key).ok())
            .collect();
        status::response(&state, &self.config, &attestations).encode()
    }
}

/// A block as getBlocks lists it, without its transactions when only headers are asked for.
fn listed(block: &Block, headers_only: bool) -> ListedBlock {
    let header = StatusHeaderData::from_block(block);
    ListedBlock {
        header: BlockHeader {
            id: Some(header.id),
            version: Some(header.version),
            timestamp: Some(header.timestamp),
            previous_block: Some(header.previous_block),
            height: Some(header.height),
            number_of_transactions: Some(header.number_of_transactions),
            payload_length: Some(header.payload_length),
            payload_hash: Some(header.payload_hash),
            generator_public_key: Some(header.generator_public_key),
            block_signature: Some(header.block_signature),
            ..BlockHeader::default()
        },
        total_amount: Amount::from(header.total_amount),
        total_fee: Amount::from(header.total_fee),
        reward: Amount::from(header.reward),
        transactions: match headers_only {
            true => Vec::new(),
            false => (block.transactions().iter())
                .map(|transaction| transaction.bytes().to_vec())
                .collect(),
        },
    }
}

/// The standalone mock peer: `mock-peer --listen <ip:port> --chain <dir>
/// [--empty-first-getblocks]`, serving a corpus chain until it is killed. The chain directory
/// holds `crypto/{network,milestones,genesisBlock}.json` and `blocks.jsonl` (one `{"hex": ...}`
/// per block after the genesis block), as a corpus does. It prints the address it listens on.
pub async fn run_standalone(args: Vec<String>) -> Result<(), MockPeerError> {
    let usage = "usage: mock-peer --listen <ip:port> --chain <dir> [--empty-first-getblocks]";
    let (mut listen, mut dir, mut empty_first) = (None, None, false);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().and_then(|address| address.parse().ok()),
            "--chain" => dir = args.next(),
            "--empty-first-getblocks" => empty_first = true,
            _ => return Err(MockPeerError::Usage(usage.into())),
        }
    }
    let (Some(listen), Some(dir)) = (listen, dir) else {
        return Err(MockPeerError::Usage(usage.into()));
    };
    let mut chain = load_chain(Path::new(&dir))?;
    chain.empty_first_get_blocks = empty_first;
    let peer = MockPeer::start(listen, Behaviour::serving(chain)).await?;
    println!("mock-peer: listening on {}", peer.address());
    std::future::pending::<()>().await;
    Ok(())
}

/// A corpus chain, with the status configuration its network gives.
fn load_chain(dir: &Path) -> Result<ServedChain, MockPeerError> {
    let failed = |file: &str, error: String| {
        MockPeerError::Usage(format!("{}: {error}", dir.join(file).display()))
    };
    let read = |file: &str| {
        std::fs::read_to_string(dir.join(file)).map_err(|error| failed(file, error.to_string()))
    };
    let network = Network::load(&read("crypto/network.json")?)
        .map_err(|error| failed("crypto/network.json", error.to_string()))?;
    let milestones = Milestones::load(&read("crypto/milestones.json")?, &network)
        .map_err(|error| failed("crypto/milestones.json", error.to_string()))?;
    let genesis = serde_json::from_str(&read("crypto/genesisBlock.json")?)
        .map_err(|error| error.to_string())
        .and_then(|json| factory::from_json(&json, &milestones).map_err(|e| e.to_string()))
        .map_err(|error| failed("crypto/genesisBlock.json", error))?;
    let mut blocks = vec![genesis];
    for (index, line) in read("blocks.jsonl")?.lines().enumerate() {
        let block = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|record| hex(record.get("hex")?.as_str()?))
            .and_then(|bytes| Block::decode_trusted(&bytes, &milestones).ok())
            .ok_or_else(|| {
                failed(
                    "blocks.jsonl",
                    format!("line {} is not a block", index.saturating_add(1)),
                )
            })?;
        blocks.push(block);
    }
    let config = PeerConfig {
        version: "4.3.1".into(),
        network: status::NetworkConfig {
            version: network.pub_key_hash,
            name: network.name,
            nethash: network.nethash,
            explorer: network.explorer,
            token: status::TokenConfig {
                name: network.token,
                symbol: network.symbol,
            },
        },
        plugins: Vec::new(),
    };
    Ok(ServedChain {
        blocks,
        milestones,
        config,
        signers: Vec::new(),
        peers: Vec::new(),
        empty_first_get_blocks: false,
    })
}

fn hex(text: &str) -> Option<Vec<u8>> {
    heartwood_crypto::utils::hex::decode(text).ok()
}
