//! The mock peer: a tokio listener on one IP that answers the client's requests from a script,
//! or serves a chain.
//!
//! The connector keeps one session per IP, so tests that need several peers bind each mock peer
//! to its own loopback address (127.0.0.1, 127.0.0.2, ...).

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use heartwood_crypto::{Block, KeyPair, Milestones};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::Heartbeat;
use heartwood_p2p_wire::replies::PeerAddress;
use heartwood_p2p_wire::status::PeerConfig;
use indexmap::IndexMap;
use thiserror::Error;

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
    /// Wait, then do the inner reply.
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
/// its blocks, getPeers from `peers`.
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
#[derive(Debug)]
pub struct MockPeer {
    address: SocketAddr,
}

impl MockPeer {
    /// Start a peer on `ip`, on a free port, that answers from `script`.
    pub async fn scripted(ip: IpAddr, script: Script) -> Result<MockPeer, MockPeerError> {
        let _ = (ip, script);
        todo!("start a scripted peer")
    }

    /// Start a peer on `ip`, on a free port, that serves `chain`.
    pub async fn serving(ip: IpAddr, chain: ServedChain) -> Result<MockPeer, MockPeerError> {
        let _ = (ip, chain);
        todo!("start a peer serving a chain")
    }

    /// The address the peer listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The requests received so far, in order: the route and the payload.
    pub fn requests(&self) -> Vec<(RouteId, Vec<u8>)> {
        todo!("the requests received")
    }

    /// How many connections the peer has accepted.
    pub fn connections(&self) -> usize {
        todo!("the connections accepted")
    }
}

/// The standalone mock peer: `mock-peer --listen <ip:port> --chain <dir>
/// [--empty-first-getblocks]`, serving a corpus chain until it is killed.
pub async fn run_standalone(args: Vec<String>) -> Result<(), MockPeerError> {
    let _ = args;
    todo!("serve a corpus chain")
}
