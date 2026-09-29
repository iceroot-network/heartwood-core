//! The server's nes session fed a script of client frames and delays: no script may panic it, the
//! session ends within a bounded time, and when it ends the socket it does so with one of the
//! reasons the reference's server gives (the ban reason), or it closes the socket without one.
//!
//! The session runs on tokio's paused clock over an in-memory connection, with the fakes of the
//! server's neighbours; a request passes the real plugin chain. The client's handshake is always
//! valid; the script's steps, each chosen by one byte, are raw bytes, a masked frame of any
//! opcode and flags, a well-formed nes message in one binary frame (a hello, a ping, a request on
//! a known or unknown route, an error), a pause, or the end of the client's stream. A session that
//! is still open after the script is left alone until the heartbeat closes it.

#![no_main]

use std::net::IpAddr;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use heartwood_crypto::{Milestones, Network};
use heartwood_p2p::devnet::Addressing;
use heartwood_p2p::server::ServerOptions;
use heartwood_p2p::server::ban_hammer::BanHammer;
use heartwood_p2p::server::listener::{self, Upgrade};
use heartwood_p2p::server::plugins::{PluginParts, Plugins};
use heartwood_p2p::server::seams::SessionId;
use heartwood_p2p::server::session::{ServerSession, SessionEnd, SessionParts};
use heartwood_p2p::testing::server::{FakeChain, FakeNode, FakePeers, StubHandlers};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Kind, Message};
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// The script's reader: bytes run out as zeros.
struct Script<'a>(std::slice::Iter<'a, u8>);

impl Script<'_> {
    fn byte(&mut self) -> u8 {
        self.0.next().copied().unwrap_or(0)
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// A client frame with `first` (FIN, RSV and opcode), masked, around `payload`.
fn frame(first: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut frame = vec![first];
    match payload.len() {
        n if n < 126 => frame.push(0x80 | n as u8),
        n => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    frame
}

/// The reasons the reference's server ends a socket with.
fn is_reference_reason(reason: &str) -> bool {
    const EXACT: [&str; 17] = [
        "Invalid message received",
        "Invalid payload received",
        "Malicious ping frame received",
        "Malicious pong frame received",
        "Message missing id",
        "Exceeded ping limit",
        "Connection already initialised",
        "Connection is not initialised",
        "Invalid subscription payload",
        "Exceeded socket rate limit",
        "Unknown message type",
        "Message missing path",
        "Not Found",
        "Max payload size exceeded",
        "Unsupported websocket frame - payload length > 2^53 - 1",
        "An internal server error occurred",
        "null",
    ];
    const PREFIXES: [&str; 5] = [
        "Invalid websocket frame - ",
        "Incorrect protocol version (expected 2 but received ",
        "Payload deserialising failed: ",
        "Payload content length greater than maximum allowed: ",
        "Validation failed",
    ];
    EXACT.contains(&reason) || PREFIXES.iter().any(|prefix| reason.starts_with(prefix))
}

/// A devnet network and milestones with a version floor, loaded once.
static DEVNET: LazyLock<(Milestones, Network)> = LazyLock::new(devnet);

fn devnet() -> (Milestones, Network) {
    let network = serde_json::json!({
        "name": "devnet", "messagePrefix": "devnet message:\n", "pubKeyHash": 90,
        "bip32": { "public": 70_617_039, "private": 70_615_956 }, "nethash": "00".repeat(32),
        "wif": 252, "slip44": 1, "client": { "token": "dROOT", "symbol": "dRT", "explorer": "" },
    });
    let network = Network::load(&network.to_string()).unwrap();
    let milestones = serde_json::json!([{
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

fuzz_target!(|bytes: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(run(bytes));
});

async fn run(bytes: &[u8]) {
    let mut script = Script(bytes.iter());
    let forger = IpAddr::from([127, 0, 0, 1]);
    let ip = if script.byte() % 4 == 0 {
        forger
    } else {
        IpAddr::from([192, 0, 2, 7])
    };
    let (milestones, network) = DEVNET.clone();
    let node = Arc::new(FakeNode::new(
        FakeChain::at(5, milestones, network).unwrap(),
    ));
    node.set_booted(script.byte() % 4 != 0);
    let options = ServerOptions {
        host: IpAddr::from([127, 0, 0, 1]),
        port: 4002,
        ban_seconds: 10,
        remote_access: vec![forger],
        minimum_versions: vec!["^3.2".to_owned()],
        rate_limit: 100,
        rate_limit_post_transactions: 25,
        max_transactions_per_request: 40,
        max_same_subnet_peers: 256,
        await_block_bound: Duration::from_secs(8),
        attestation_wait: Duration::from_millis(500),
        addressing: Addressing::Off,
    };
    let plugins = Arc::new(Plugins::new(PluginParts {
        options: Arc::new(options),
        node,
        peers: Arc::new(FakePeers::default()),
        handlers: Arc::new(StubHandlers::default()),
        ban_hammer: Arc::new(BanHammer::new(10, vec![forger])),
        clock: Arc::new(heartwood_kernel::SystemClock),
    }));

    let (mut client, served) = tokio::io::duplex(1024 * 1024);
    let session = tokio::spawn(async move {
        let Upgrade::Open(reader, writer) = listener::upgrade(Box::new(served)).await else {
            return SessionEnd::Handshake;
        };
        let (_orders, control) = mpsc::unbounded_channel();
        let parts = SessionParts {
            id: SessionId(1),
            ip,
            plugins,
            beats_from: tokio::time::Instant::now(),
            control,
        };
        ServerSession::new(parts).run(reader, writer).await
    });
    let handshake = "GET / HTTP/1.1\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\
                     Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
    if client.write_all(handshake.as_bytes()).await.is_err() {
        return;
    }
    let (mut from_server, mut to_server) = tokio::io::split(client);
    tokio::spawn(async move {
        let mut sink = Vec::new();
        while from_server.read_buf(&mut sink).await.is_ok_and(|n| n > 0) {
            sink.clear();
        }
    });

    let routes = RouteId::ALL;
    let mut steps = 0;
    while script.0.len() > 0 && steps < 64 {
        steps += 1;
        let written = match script.byte() % 5 {
            0 => {
                let n = usize::from(script.byte());
                to_server.write_all(&script.bytes(n)).await
            }
            1 => {
                let first = script.byte();
                let n = usize::from(script.byte());
                let mask = [script.byte(), script.byte(), script.byte(), script.byte()];
                to_server
                    .write_all(&frame(first, &script.bytes(n), mask))
                    .await
            }
            2 => {
                let kind = [Kind::Hello, Kind::Ping, Kind::Request, Kind::Error]
                    [usize::from(script.byte() % 4)];
                let version = script.byte() % 4;
                let id = u32::from(script.byte() % 8);
                let route = routes[usize::from(script.byte()) % routes.len()];
                let n = usize::from(script.byte());
                let mut message = Message {
                    kind,
                    version,
                    ..Message::request(id, route, script.bytes(n))
                };
                if script.byte() % 8 == 0 {
                    message.path = String::from_utf8_lossy(&script.bytes(4)).into_owned();
                }
                match message.encode() {
                    Ok(bytes) => {
                        to_server
                            .write_all(&frame(0x82, &bytes, [1, 2, 3, 4]))
                            .await
                    }
                    Err(_) => Ok(()),
                }
            }
            3 => {
                tokio::time::sleep(Duration::from_millis(u64::from(script.byte()) * 200)).await;
                Ok(())
            }
            _ => to_server.shutdown().await,
        };
        if written.is_err() {
            break;
        }
    }
    // Every session ends: the heartbeat closes a silent socket within 35 s, and a close waits
    // 30 s at most for the client's close frame.
    let end = tokio::time::timeout(Duration::from_secs(120), session).await;
    let end = end
        .expect("the session ends")
        .expect("the session does not panic");
    if let SessionEnd::Terminated { reason } = &end {
        assert!(
            is_reference_reason(reason),
            "a reason the reference never gives: {reason}"
        );
    }
}
