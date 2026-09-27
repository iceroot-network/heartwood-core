//! Test support for the forger's tests: the devnet milestones and keys, a relay's answers, the
//! scripted relay over the peer network's mock peer, the relay client over in-memory
//! connections, a clock on tokio's paused time, and the log lines a test produced.
//!
//! Every relay a test runs listens in the forger tests' loopback block, 127.10.0.0/24; the client
//! reaches it through the mock peer's in-memory connections, which tokio's paused clock can drive.

#![allow(dead_code, reason = "each test file uses its own part of the support")]

use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::{Aux, Block, KeyPair, Milestones, Network};
use heartwood_forger::client::{Client, ClientOptions};
use heartwood_forger::options::RelayHost;
use heartwood_kernel::Clock;
use heartwood_logger::{Level, LineLayer};
use heartwood_p2p::testing::{MockPeer, Reply, Script};
use heartwood_p2p::transport::Dial;
use heartwood_p2p_wire::codecs::{blocks, transactions};
use heartwood_p2p_wire::internal::{
    self, CurrentRound, DelegateWallet, InternalBody, LastBlockData, NetworkState,
    NetworkStateStatus, QuorumDetails,
};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::transactions::GetUnconfirmedTransactionsResponse;
use heartwood_p2p_wire::status::{
    self, NetworkConfig, PeerConfig, ReceivedStatus, SignedState, StatusHeaderData, TokenConfig,
};
use serde_json::{Map, Value, json};
use tokio::time::Instant;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The version the forger's public calls carry in the tests.
pub const VERSION: &str = "4.3.1-heartwood.0.1.0";
/// The relays' port.
pub const PORT: u16 = 6002;
/// The devnet's epoch, in Unix milliseconds: 2026-01-01T00:00:00.000Z.
pub const EPOCH_MS: i64 = 1_767_225_600_000;
/// The devnet's block time.
pub const BLOCK_TIME: u64 = 8;

/// Host `n` of the tests' loopback block.
pub fn ip(n: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(127, 10, 0, n))
}

/// Relay host `n` of the tests' loopback block, on [`PORT`].
pub fn host(n: u8) -> RelayHost {
    RelayHost {
        hostname: ip(n),
        port: PORT,
    }
}

pub fn network() -> Network {
    Network {
        name: "devnet".into(),
        message_prefix: "devnet message:\n".into(),
        address_character: Some("d".into()),
        bip32_public: 70_617_039,
        bip32_private: 70_615_956,
        pub_key_hash: 90,
        nethash: "0".repeat(64),
        wif: 252,
        slip44: 1,
        token: "dROOT".into(),
        symbol: "dRT".into(),
        explorer: String::new(),
    }
}

/// The devnet milestones: 53 delegates, 8-second blocks.
pub fn milestones() -> Milestones {
    let milestone = json!({
        "height": 1,
        "activeDelegates": 53,
        "blockTime": BLOCK_TIME,
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
        "epoch": "2026-01-01T00:00:00.000Z",
        "reward": 0,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 },
        "donations": {},
        "dynamicReward": { "enabled": false },
        "transfer": { "maximum": 256, "minimum": 1 },
    });
    Milestones::load(&json!([milestone]).to_string(), &network()).expect("the milestones load")
}

/// Delegate `n`'s key pair.
pub fn key(n: u32) -> KeyPair {
    KeyPair::from_passphrase(&format!("forger test delegate {n}")).expect("a key pair")
}

/// Delegate `n`'s public key, in hex.
pub fn hex(n: u32) -> String {
    key(n).public_key().to_compressed().to_hex()
}

/// Delegate `n`'s wallet, `genesis_<n>`, as a round lists it.
pub fn wallet(n: u32) -> DelegateWallet {
    let fields = json!({
        "address": format!("D{n}"),
        "publicKey": hex(n),
        "balance": "0",
        "nonce": "0",
        "delegate": { "username": format!("genesis_{n}"), "voteBalance": "0", "rank": n },
    });
    let Value::Object(fields) = fields else {
        unreachable!("an object")
    };
    DelegateWallet::new(fields).expect("a wallet")
}

/// A round at `timestamp` (seconds after the epoch) over the tip at `height`: `delegates` are
/// the active ones in forging order, `registered` the others; `current` and `next` index the
/// active ones.
pub fn round(
    timestamp: i64,
    height: u32,
    delegates: &[u32],
    registered: &[u32],
    current: Option<usize>,
    next: Option<usize>,
) -> CurrentRound {
    let active: Vec<DelegateWallet> = delegates.iter().map(|n| wallet(*n)).collect();
    let pick = |index: Option<usize>| index.and_then(|i| active.get(i).cloned());
    let mut last_block = Map::new();
    last_block.insert("height".into(), json!(height));
    last_block.insert("id".into(), json!(block_id(height)));
    CurrentRound {
        all_delegates: (delegates.iter().chain(registered))
            .map(|n| wallet(*n))
            .collect(),
        can_forge: true,
        current: 1,
        current_forger: pick(current),
        next_forger: pick(next),
        delegates: active.clone(),
        last_block: LastBlockData::new(last_block).expect("a block"),
        reward: "0".into(),
        timestamp,
    }
}

/// The id the tests give the block at `height`.
pub fn block_id(height: u32) -> String {
    format!("{height:064x}")
}

/// A network state at `height` whose quorum forges: all 53 delegates agree.
pub fn state(height: u32) -> NetworkState {
    NetworkState {
        status: NetworkStateStatus::Default,
        node_height: Some(height),
        last_block_id: Some(block_id(height)),
        last_generator: None,
        last_slot_number: Some(0),
        quorum_details: QuorumDetails {
            has_quorum: 53,
            ..QuorumDetails::default()
        },
    }
}

/// A status whose last block has `timestamp` and was forged by `generator` (hex).
pub fn status(height: u32, timestamp: u32, generator: &str) -> ReceivedStatus {
    let state = SignedState {
        height,
        forging_allowed: true,
        current_slot: 0,
        header: StatusHeaderData {
            id: block_id(height),
            version: 0,
            timestamp,
            previous_block: block_id(height.saturating_sub(1)),
            height,
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 0,
            payload_length: 0,
            payload_hash: "00".repeat(32),
            generator_public_key: generator.into(),
            block_signature: "00".repeat(64),
        },
    };
    let config = PeerConfig {
        version: VERSION.into(),
        network: NetworkConfig {
            version: 90,
            name: "devnet".into(),
            nethash: "0".repeat(64),
            explorer: String::new(),
            token: TokenConfig {
                name: "dROOT".into(),
                symbol: "dRT".into(),
            },
        },
        plugins: Vec::new(),
    };
    let bytes = status::response(&state, &config, &[]).encode();
    status::decode_response(&bytes).expect("a status decodes")
}

/// An empty block of delegate `n` at `height`, forged at `timestamp`.
pub fn block(n: u32, height: u32, timestamp: u32) -> Block {
    let draft = BlockDraft {
        version: 0,
        timestamp,
        height,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: heartwood_crypto::crypto::hash::sha256(b""),
    };
    factory::make(
        draft,
        Vec::new(),
        &key(n),
        Aux::fixed([7; 32]),
        &milestones(),
    )
    .expect("a block")
}

/// An internal route's answer.
pub fn json<T: InternalBody>(body: &T) -> Reply {
    Reply::Payload(internal::encode(body))
}

/// An answer of raw text.
pub fn text(body: &str) -> Reply {
    Reply::Payload(body.as_bytes().to_vec())
}

/// The status answer of [`status`].
pub fn status_reply(height: u32, timestamp: u32, generator: &str) -> Reply {
    Reply::Payload(status::encode_response(&status(
        height, timestamp, generator,
    )))
}

/// A pool answer with these transactions, of `pool_size` in all.
pub fn pool_reply(pool_size: u32, listed: Vec<Vec<u8>>) -> Reply {
    let answer = transactions::WithTransactions {
        message: GetUnconfirmedTransactionsResponse {
            pool_size: Some(pool_size),
            ..GetUnconfirmedTransactionsResponse::default()
        },
        transactions: listed,
    };
    Reply::Payload(transactions::get_unconfirmed_transactions_encode_response(
        &answer,
    ))
}

/// The answer to a posted block.
pub fn posted_reply(height: u32) -> Reply {
    Reply::Payload(blocks::post_block_encode_response(&PostBlockResponse {
        status: Some(true),
        height: Some(height),
        ..PostBlockResponse::default()
    }))
}

/// A relay's graceful refusal before its blockchain has booted.
pub fn not_ready() -> Reply {
    Reply::Error {
        status: 499,
        payload: br#"{"error":"Unknown","message":"App is not ready"}"#.to_vec(),
    }
}

/// A scripted relay at host `n`.
pub async fn relay(n: u8, script: Script) -> MockPeer {
    MockPeer::scripted(ip(n), script)
        .await
        .expect("the mock relay listens")
}

/// A client of `hosts` over in-memory connections to `relays`; a host of no relay refuses.
pub fn client(hosts: &[RelayHost], relays: &[&MockPeer]) -> Client {
    client_over(hosts, MockPeer::dial(relays))
}

/// A client of `hosts` over `dial`.
pub fn client_over(hosts: &[RelayHost], dial: Dial) -> Client {
    let options = ClientOptions {
        hosts: hosts.to_vec(),
        local_address: None,
        version: VERSION.into(),
    };
    Client::with_dial(options, dial)
}

/// A clock on tokio's time: `start_ms` when it is made, then as tokio's clock (which the tests
/// pause) moves.
pub struct TokioClock {
    start: Instant,
    start_ms: i64,
}

impl TokioClock {
    pub fn at(start_ms: i64) -> Arc<TokioClock> {
        Arc::new(TokioClock {
            start: Instant::now(),
            start_ms,
        })
    }
}

impl Clock for TokioClock {
    fn now_ms(&self) -> i64 {
        let elapsed = i64::try_from(self.start.elapsed().as_millis()).expect("a short test");
        self.start_ms + elapsed
    }
}

/// Unix milliseconds `ms` into slot `slot`.
pub fn in_slot(slot: i64, ms: i64) -> i64 {
    EPOCH_MS + slot * 8_000 + ms
}

/// The log lines of a test, as `LEVEL: message` without the time and the emoji codes.
#[derive(Clone, Default)]
pub struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Lines {
    /// Capture the lines logged on this thread until the guard is dropped.
    pub fn capture() -> (Lines, impl Drop) {
        let lines = Lines::default();
        let clock = Arc::new(heartwood_kernel::ManualClock::new(EPOCH_MS));
        let layer =
            LineLayer::new(false, clock).output(Level::Debug, false, Box::new(lines.clone()));
        let guard = tracing_subscriber::registry().with(layer).set_default();
        (lines, guard)
    }

    /// Every line so far.
    pub fn all(&self) -> Vec<String> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).expect("UTF-8 lines");
        (text.lines())
            .map(|line| {
                line.split_once("] ")
                    .map_or(line, |(_, rest)| rest)
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// The lines that contain `part`.
    pub fn with(&self, part: &str) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|line| line.contains(part))
            .collect()
    }

    /// Forget the lines so far.
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// Wait `ms` milliseconds of tokio's time.
pub async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}
