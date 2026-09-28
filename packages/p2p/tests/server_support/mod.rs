//! What the server's tests share: a devnet network and milestones, a bench with a bound server
//! over the fakes of `heartwood_p2p::testing::server`, the client's session dialled through the
//! server's in-memory accept, a raw WebSocket client for frames no client sends, and a session
//! run on its own for the reason it ends with.
//!
//! The tests run on tokio's paused clock: every connection is an in-memory pipe.

#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use heartwood_crypto::{Milestones, Network};
use heartwood_kernel::ManualClock;
use heartwood_p2p::devnet::Addressing;
use heartwood_p2p::errors::SessionError;
use heartwood_p2p::nes::{ConnectOptions, Session};
use heartwood_p2p::server::ban_hammer::BanHammer;
use heartwood_p2p::server::listener::{self, Upgrade};
use heartwood_p2p::server::plugins::{PluginParts, Plugins};
use heartwood_p2p::server::seams::SessionId;
use heartwood_p2p::server::session::{ServerSession, SessionEnd, SessionParts};
use heartwood_p2p::server::{P2pServer, ServerOptions, ServerParts};
use heartwood_p2p::testing::server::{
    FakeChain, FakeNode, FakePeers, RecordedEvents, StubHandlers,
};
use heartwood_p2p::transport::{Dial, Stream};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::peer::GetStatusRequest;
use heartwood_p2p_wire::messages::shared::Headers;
use heartwood_p2p_wire::nes::Message;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The version the tests' clients report.
pub const VERSION: &str = "4.3.1-heartwood.0.1.0";
/// The forger's address in the tests' `remoteAccess`.
pub const FORGER: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 11, 0, 200));
/// The tests' clock at the start, in milliseconds since the Unix epoch.
pub const START_MS: i64 = 1_800_000_000_000;

/// A peer's address in the tests' loopback block.
pub fn peer(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 11, 0, last))
}

/// A devnet network and milestones: the version floor `>=4.3.0` from height 1.
pub fn devnet() -> (Milestones, Network) {
    let network = Network::load(
        &json!({
            "name": "devnet", "messagePrefix": "devnet message:\n", "pubKeyHash": 90,
            "bip32": { "public": 70_617_039, "private": 70_615_956 }, "nethash": "00".repeat(32),
            "wif": 252, "slip44": 1, "client": { "token": "dROOT", "symbol": "dRT", "explorer": "" },
        })
        .to_string(),
    )
    .unwrap();
    let milestones = json!([{
        "height": 1, "activeDelegates": 53, "blockTime": 8, "epoch": "2026-01-01T00:00:00.000Z",
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 }, "reward": 0,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 }, "donations": {},
        "dynamicReward": { "enabled": false }, "transfer": { "maximum": 256, "minimum": 1 },
        "p2p": { "minimumVersions": [">=4.3.0"] },
    }]);
    (
        Milestones::load(&milestones.to_string(), &network).unwrap(),
        network,
    )
}

/// The server's options on the tests' block, with the forger at [`FORGER`].
pub fn options() -> ServerOptions {
    ServerOptions {
        host: peer(1),
        port: 0,
        ban_seconds: 10,
        remote_access: vec![FORGER],
        minimum_versions: vec!["^3.2".to_owned()],
        rate_limit: 100,
        rate_limit_post_transactions: 25,
        max_transactions_per_request: 40,
        max_same_subnet_peers: 256,
        await_block_bound: Duration::from_secs(8),
        attestation_wait: Duration::from_millis(500),
        addressing: Addressing::Off,
    }
}

/// A getStatus request's payload with `version`.
pub fn status_request(version: &str) -> Vec<u8> {
    let headers = Headers {
        version: Some(version.to_owned()),
        port: Some(4002),
        ..Headers::default()
    };
    let request = GetStatusRequest {
        headers: Some(headers),
        ..GetStatusRequest::default()
    };
    request.encode()
}

/// The fakes around a server.
pub struct Fakes {
    pub node: Arc<FakeNode>,
    pub peers: Arc<FakePeers>,
    pub handlers: Arc<StubHandlers>,
    pub events: Arc<RecordedEvents>,
    pub clock: Arc<ManualClock>,
}

impl Fakes {
    pub fn new() -> Fakes {
        let (milestones, network) = devnet();
        Fakes {
            node: Arc::new(FakeNode::new(
                FakeChain::at(10, milestones, network).unwrap(),
            )),
            peers: Arc::default(),
            handlers: Arc::default(),
            events: Arc::default(),
            clock: Arc::new(ManualClock::new(START_MS)),
        }
    }

    /// A plugin chain over the fakes, with `options` and `bans`.
    pub fn plugins(&self, options: ServerOptions, bans: Arc<BanHammer>) -> Plugins {
        Plugins::new(PluginParts {
            options: Arc::new(options),
            node: self.node.clone(),
            peers: self.peers.clone(),
            handlers: self.handlers.clone(),
            ban_hammer: bans,
            clock: self.clock.clone(),
        })
    }
}

/// A bound server over the fakes.
pub struct Bench {
    pub server: Arc<P2pServer>,
    pub fakes: Fakes,
}

impl Bench {
    pub async fn new() -> Bench {
        Bench::with(options()).await
    }

    pub async fn with(options: ServerOptions) -> Bench {
        let fakes = Fakes::new();
        let parts = ServerParts {
            options,
            node: fakes.node.clone(),
            peers: fakes.peers.clone(),
            handlers: fakes.handlers.clone(),
            events: fakes.events.clone(),
        };
        let server = P2pServer::bind_with_clock(parts, fakes.clock.clone())
            .await
            .unwrap();
        Bench {
            server: Arc::new(server),
            fakes,
        }
    }

    /// Streams to the server that it sees from `ip`.
    pub fn dial(&self, ip: IpAddr) -> Dial {
        let server = Arc::clone(&self.server);
        Arc::new(move |_| {
            let (client, served) = tokio::io::duplex(64 * 1024 * 1024);
            server.accept(Box::new(served), ip);
            Box::pin(async move { Ok(Box::new(client) as Box<dyn Stream>) })
        })
    }

    /// The client's session from `ip`, connected, with the forger's receive limit.
    pub async fn session(&self, ip: IpAddr) -> Session {
        let options = ConnectOptions {
            retries: 0,
            max_payload: 20 * 1024 * 1024,
            ..ConnectOptions::default()
        };
        let address = SocketAddr::new(peer(1), 4002);
        let hook = Arc::new(|_: SessionError| {});
        let (session, connected) = Session::connect(address, options, hook, self.dial(ip));
        connected.await.unwrap();
        session
    }

    /// A raw client from `ip`, its WebSocket open, or `None` when the server drops the
    /// connection first.
    pub async fn raw(&self, ip: IpAddr) -> Option<Raw> {
        let (client, served) = tokio::io::duplex(64 * 1024 * 1024);
        self.server.accept(Box::new(served), ip);
        Raw::open(client).await
    }
}

/// A client that writes and reads raw WebSocket frames.
pub struct Raw {
    stream: DuplexStream,
    buffer: Vec<u8>,
}

/// A frame the server sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    Binary(Message),
    Close(Vec<u8>),
    Ended,
}

impl Raw {
    /// The upgrade on `stream`, or `None` when the server drops it.
    pub async fn open(mut stream: DuplexStream) -> Option<Raw> {
        let request = "GET / HTTP/1.1\r\nHost: 127.11.0.1:4002\r\nConnection: Upgrade\r\n\
                       Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
                       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut buffer = Vec::new();
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            if stream.read_buf(&mut buffer).await.ok()? == 0 {
                return None;
            }
        }
        let end = buffer.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert!(buffer.starts_with(b"HTTP/1.1 101 "));
        let buffer = buffer.split_off(end);
        Some(Raw { stream, buffer })
    }

    /// Write a masked frame with `first` (FIN, RSV and opcode) and `payload`.
    pub async fn frame(&mut self, first: u8, payload: &[u8]) {
        let mut frame = vec![first];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n < 65_536 => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = [1, 2, 3, 4];
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
        self.stream.write_all(&frame).await.unwrap();
    }

    /// Write raw bytes.
    pub async fn bytes(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// Send a nes message as a binary frame.
    pub async fn send(&mut self, message: &Message) {
        self.frame(0x82, &message.encode().unwrap()).await;
    }

    /// The client's hello.
    pub async fn hello(&mut self) {
        self.send(&Message::client_hello(1)).await;
        assert!(matches!(self.next().await, Sent::Binary(reply) if reply.id == 1));
    }

    /// A request on `route` with `id` and `payload`.
    pub async fn request(&mut self, id: u32, route: RouteId, payload: Vec<u8>) {
        self.send(&Message::request(id, route, payload)).await;
    }

    /// The server's next frame.
    pub async fn next(&mut self) -> Sent {
        loop {
            if let [first, second, rest @ ..] = self.buffer.as_slice() {
                assert_eq!(second & 0x80, 0, "the server masks nothing");
                let (length, at) = match second & 0x7f {
                    126 if rest.len() >= 2 => (u16::from_be_bytes([rest[0], rest[1]]) as usize, 4),
                    127 if rest.len() >= 8 => (
                        u64::from_be_bytes(rest[..8].try_into().unwrap()) as usize,
                        10,
                    ),
                    126 | 127 => (usize::MAX, 0),
                    short => (short as usize, 2),
                };
                if length != usize::MAX && self.buffer.len() >= at + length {
                    let opcode = first & 0x0f;
                    let payload: Vec<u8> = self.buffer.drain(..at + length).skip(at).collect();
                    return match opcode {
                        2 => Sent::Binary(Message::parse(&payload).unwrap()),
                        8 => Sent::Close(payload),
                        other => panic!("unexpected opcode {other}"),
                    };
                }
            }
            match self.stream.read_buf(&mut self.buffer).await {
                Ok(0) | Err(_) => return Sent::Ended,
                Ok(_) => {}
            }
        }
    }
}

/// The reason a session from `ip` ends with after the client's handshake and `frames` (each
/// written as it is), with the fakes' plugin chain; the session is left alone after the frames
/// for up to two minutes.
pub async fn session_end(fakes: &Fakes, ip: IpAddr, frames: &[Vec<u8>]) -> SessionEnd {
    let (client, served) = tokio::io::duplex(64 * 1024 * 1024);
    let bans = Arc::new(BanHammer::new(10, vec![FORGER]));
    let plugins = Arc::new(fakes.plugins(options(), bans));
    let serving = tokio::spawn(async move {
        let Upgrade::Open(reader, writer) = listener::upgrade(Box::new(served)).await else {
            return SessionEnd::Handshake;
        };
        let (_orders, control) = mpsc::unbounded_channel();
        let parts = SessionParts {
            id: SessionId(1),
            ip,
            plugins,
            beats_from: Instant::now(),
            control,
        };
        ServerSession::new(parts).run(reader, writer).await
    });
    let mut raw = Raw::open(client).await.unwrap();
    for frame in frames {
        raw.bytes(frame).await;
    }
    let draining = tokio::spawn(async move { while raw.next().await != Sent::Ended {} });
    let end = tokio::time::timeout(Duration::from_secs(120), serving).await;
    draining.abort();
    end.expect("the session ends").unwrap()
}

/// A masked client frame with `first` and `payload`, as bytes.
pub fn frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![first];
    match payload.len() {
        n if n < 126 => frame.push(0x80 | n as u8),
        n if n < 65_536 => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&[0; 4]);
    frame.extend_from_slice(payload);
    frame
}

/// A nes message as a masked binary frame.
pub fn binary(message: &Message) -> Vec<u8> {
    frame(0x82, &message.encode().unwrap())
}
