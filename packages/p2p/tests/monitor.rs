//! The network monitor over fakes of the client, the acceptance and the peer rules, with tokio's
//! paused clock and a manual wall clock: the cadences of the network status and of `pingAll`, the
//! discovery thresholds, the cleanse race and its infractions, the download plan and its chunk
//! cache over several peers and chunk sizes, the transaction download's memory, the broadcast
//! delay, the fork health, seeds and sources, the port probes against a local HTTP listener, and
//! a downloaded block rebuilt as the reference's block factory rebuilds it.
//!
//! The case with the real communicator, connector and verifier against three mock peers is an
//! integration test, run once every part of the client is written.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::TransferItem;
use heartwood_crypto::utils::hex;
use heartwood_crypto::validation::Mode;
use heartwood_crypto::{
    Address, Aux, Block, BlockId, KeyPair, Milestones, Network, Params, PublicKeyBytes,
    Transaction, TransactionId,
};
use heartwood_kernel::{Event, ManualClock};
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::download::read_listed;
use heartwood_p2p::errors::{AssembleError, DownloadError, PingError, SeedError};
use heartwood_p2p::monitor::{CleanseOptions, MonitorParts, NetworkStatus};
use heartwood_p2p::peer::{Peer, PeerState, SharedPeer, Verification};
use heartwood_p2p::processor::AcceptOptions;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules, PortProbe};
use heartwood_p2p::seeds::HttpProbe;
use heartwood_p2p::view::{BlockPing, ChainView, DelegateInfo, PeerEvents};
use heartwood_p2p::{
    BlocksRequest, DownloadedBlock, NetworkMonitor, P2pOptions, PeerRepository, to_block,
};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::errors::ReplyRefused;
use heartwood_p2p_wire::messages::blocks::{BlockHeader as ListedHeader, PostBlockResponse};
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{Instant, advance, sleep};

/// 2026-01-01T00:00:00Z, the devnet epoch, in milliseconds.
const EPOCH_MS: i64 = 1_767_225_600_000;

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 0, 2, last])
}

fn address(last: u8) -> PeerAddress {
    PeerAddress {
        ip: format!("198.51.100.{last}"),
        port: 4002,
    }
}

fn network() -> Network {
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

fn milestones() -> Milestones {
    let milestone = json!({
        "height": 1,
        "activeDelegates": 53,
        "blockTime": 8,
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

fn keys(n: u32) -> KeyPair {
    KeyPair::from_passphrase(&format!("monitor test {n}")).expect("keys")
}

/// A signed transfer with nonce `nonce`.
fn transfer(nonce: u64, params: &Params) -> Transaction {
    let recipient = Address::from_public_key(keys(0).public_key(), 90);
    TransactionBuilder::transfer(vec![TransferItem {
        amount: 1_000,
        recipient,
    }])
    .nonce(nonce)
    .fee(10_000_000)
    .build(&keys(1), Aux::random(), params)
    .expect("a transfer")
}

/// A block at `height` with `transactions`, signed by `keys(2)`.
fn block(height: u32, transactions: Vec<Transaction>, milestones: &Milestones) -> Block {
    let count = u32::try_from(transactions.len()).unwrap();
    let draft = BlockDraft {
        version: 0,
        timestamp: 8 * (height - 1),
        height,
        previous_block: [0xab; 32],
        number_of_transactions: count,
        total_amount: 1_000 * u64::from(count),
        total_fee: 10_000_000 * u64::from(count),
        reward: 0,
        payload_length: 32 * count,
        payload_hash: [0x11; 32],
    };
    factory::make(draft, transactions, &keys(2), Aux::random(), milestones).expect("a block")
}

/// A block as a getBlocks reply lists it, after the reply check.
fn listed(block: &Block) -> ListedBlock {
    let h = block.header();
    ListedBlock {
        header: ListedHeader {
            id: Some(block.id().to_hex()),
            version: Some(h.version),
            timestamp: Some(h.timestamp),
            previous_block: Some(hex::encode(&h.previous_block)),
            height: Some(h.height),
            number_of_transactions: Some(h.number_of_transactions),
            payload_length: Some(h.payload_length),
            payload_hash: Some(hex::encode(&h.payload_hash)),
            generator_public_key: Some(h.generator_public_key.to_hex()),
            block_signature: Some(h.block_signature.to_hex()),
            ..ListedHeader::default()
        },
        total_amount: Amount::new(h.total_amount.into()),
        total_fee: Amount::new(h.total_fee.into()),
        reward: Amount::new(h.reward.into()),
        transactions: block
            .transactions()
            .iter()
            .map(|t| t.bytes().to_vec())
            .collect(),
    }
}

/// A downloaded block of the plan tests: only its height and the peer matter.
fn stub_block(height: u32, from: IpAddr) -> DownloadedBlock {
    DownloadedBlock {
        header: ListedBlock {
            header: ListedHeader {
                height: Some(height),
                ..ListedHeader::default()
            },
            ..ListedBlock::default()
        },
        transactions: Some(Vec::new()),
        ip: from,
    }
}

fn state(height: u32) -> PeerState {
    PeerState {
        height,
        forging_allowed: false,
        current_slot: 0,
        header: Default::default(),
        total_amount: Amount::ZERO,
        total_fee: Amount::ZERO,
        reward: Amount::ZERO,
    }
}

// ---- the fakes ------------------------------------------------------------------------------

/// The node's chain: its last block, its delegates' ranks and its block ping.
struct Chain {
    milestones: Milestones,
    network: Network,
    last: Mutex<Arc<Block>>,
    ranks: HashMap<PublicKeyBytes, u32>,
    ping: Mutex<Option<BlockPing>>,
}

impl ChainView for Chain {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.last.lock().unwrap())
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
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
        self.ranks.get(key).map(|&rank| DelegateInfo {
            username: format!("d{rank}"),
            rank: Some(rank),
        })
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.ranks.keys().copied().collect()
    }
    fn is_synced(&self) -> bool {
        true
    }
    fn is_idle(&self) -> bool {
        true
    }
    fn block_ping(&self) -> Option<BlockPing> {
        self.ping.lock().unwrap().clone()
    }
}

/// How a peer answers a ping.
#[derive(Debug, Clone)]
enum Ping {
    /// At once, with this height, attesting these keys, with this verification.
    Ok(u32, Vec<PublicKeyBytes>, Option<Verification>),
    /// After a while, with this height.
    Slow(Duration, u32),
    /// A failure.
    Fail,
}

/// A request the fake client got.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Ping {
        ip: IpAddr,
        timeout: Duration,
        force: bool,
        skip: bool,
    },
    Ports(IpAddr),
    Peers(IpAddr),
    Blocks(IpAddr, BlocksRequest),
    Unconfirmed(IpAddr),
    PostBlock(IpAddr),
    PostTransactions(IpAddr, usize),
    Removed(IpAddr),
}

/// What the fake client answers.
#[derive(Default)]
struct Answers {
    pings: HashMap<IpAddr, Ping>,
    peers: HashMap<IpAddr, Vec<PeerAddress>>,
    /// Per peer, the height up to which it serves blocks.
    tops: HashMap<IpAddr, u32>,
    /// Peers and chunk starts whose getBlocks fails as the reader fails.
    failing: Vec<(IpAddr, u32)>,
    pool: HashMap<IpAddr, Vec<Vec<u8>>>,
    throttled: Vec<(IpAddr, RouteId)>,
}

struct Client {
    answers: Mutex<Answers>,
    calls: Mutex<Vec<Call>>,
    repository: Arc<PeerRepository>,
}

impl Client {
    fn log(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn blocks_calls(&self) -> Vec<(IpAddr, u32, u32)> {
        let calls = self.calls();
        let blocks = calls.iter().filter_map(|call| match call {
            Call::Blocks(ip, request) => Some((*ip, request.from_height, request.block_limit)),
            _ => None,
        });
        blocks.collect()
    }
    fn count(&self, matches: impl Fn(&Call) -> bool) -> usize {
        self.calls().iter().filter(|call| matches(call)).count()
    }
    fn answers(&self) -> std::sync::MutexGuard<'_, Answers> {
        self.answers.lock().unwrap()
    }
}

impl PeerClient for Client {
    async fn ping(
        &self,
        peer: &SharedPeer,
        timeout: Duration,
        attest: bool,
        force: bool,
        skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        assert!(attest, "a cleanse's pings attest");
        self.log(Call::Ping {
            ip: peer.ip(),
            timeout,
            force,
            skip: skip_common_blocks,
        });
        let answer = self.answers().pings.get(&peer.ip()).cloned();
        let (height, keys, verification) = match answer {
            Some(Ping::Ok(height, keys, verification)) => (height, keys, verification),
            Some(Ping::Slow(delay, height)) => {
                sleep(delay).await;
                (height, Vec::new(), None)
            }
            Some(Ping::Fail) | None => return Err(PingError::StatusResponse(peer.ip())),
        };
        peer.with(|p| {
            p.state = Some(state(height));
            p.public_keys = keys;
            p.verification = verification;
            p.stale = false;
        });
        Ok(Some(state(height)))
    }

    async fn ping_ports(&self, peer: &SharedPeer) {
        self.log(Call::Ports(peer.ip()));
    }

    async fn get_peers(&self, peer: &SharedPeer, _silent: bool) -> Option<Vec<PeerAddress>> {
        self.log(Call::Peers(peer.ip()));
        self.answers().peers.get(&peer.ip()).cloned()
    }

    async fn has_common_blocks(
        &self,
        _peer: &SharedPeer,
        _ids: &[BlockId],
        _timeout: Duration,
    ) -> Option<CommonBlock> {
        unreachable!("the monitor never asks for common blocks")
    }

    async fn get_peer_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        self.log(Call::Blocks(peer.ip(), request));
        let answers = self.answers();
        if answers.failing.contains(&(peer.ip(), request.from_height)) {
            let bytes = [0xff_u8];
            let params = milestones().at(1).clone();
            let source = Transaction::decode_trusted(&bytes, &params).unwrap_err();
            return Err(DownloadError::Transaction {
                height: request.from_height + 1,
                source,
            });
        }
        let top = answers.tops.get(&peer.ip()).copied().unwrap_or(0);
        let last = top.min(request.from_height + request.block_limit);
        let heights = (request.from_height + 1)..=last;
        Ok(heights
            .map(|height| stub_block(height, peer.ip()))
            .collect())
    }

    async fn get_unconfirmed_transactions(
        &self,
        peer: &SharedPeer,
        _exclude: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        self.log(Call::Unconfirmed(peer.ip()));
        self.answers()
            .pool
            .get(&peer.ip())
            .cloned()
            .unwrap_or_default()
    }

    async fn post_block(&self, peer: &SharedPeer, _block: &Block) {
        self.log(Call::PostBlock(peer.ip()));
    }

    fn post_transactions(&self, peer: &SharedPeer, transactions: Arc<Vec<Vec<u8>>>) {
        self.log(Call::PostTransactions(peer.ip(), transactions.len()));
    }

    fn would_throttle(&self, ip: IpAddr, route: RouteId) -> bool {
        self.answers().throttled.contains(&(ip, route))
    }

    fn disconnect(&self, _ip: IpAddr) {}

    fn remove_peer(&self, ip: IpAddr) {
        self.log(Call::Removed(ip));
        self.repository.forget(ip);
    }
}

/// The acceptance: records each candidate, whether it was still a peer, and adds it when told.
struct Acceptor {
    accepted: Mutex<Vec<(PeerAddress, AcceptOptions, bool)>>,
    adds: bool,
    repository: Arc<PeerRepository>,
}

impl Acceptor {
    fn accepted(&self) -> Vec<(PeerAddress, AcceptOptions, bool)> {
        self.accepted.lock().unwrap().clone()
    }
}

impl PeerAcceptor for Acceptor {
    fn validate_peer_ip(&self, _peer: &mut PeerAddress, _options: AcceptOptions) -> bool {
        unreachable!("the monitor never checks an address on its own")
    }

    async fn validate_and_accept(&self, peer: PeerAddress, options: AcceptOptions) {
        let parsed: IpAddr = peer.ip.parse().expect("an address");
        let known = self.repository.has(parsed);
        self.accepted
            .lock()
            .unwrap()
            .push((peer.clone(), options, known));
        if self.adds {
            self.repository
                .set(SharedPeer::new(Peer::new(parsed, peer.port)));
        }
    }
}

/// The peer rules the monitor calls: the version comparison and the version gate.
struct Rules {
    valid: Vec<String>,
}

impl PeerRules for Rules {
    fn encode_request(&self, _: &ClientRequest, _: &Headers) -> Vec<u8> {
        unreachable!()
    }
    fn check_peers(&self, _: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
        unreachable!()
    }
    fn check_common_blocks(
        &self,
        _: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused> {
        unreachable!()
    }
    fn check_blocks(&self, _: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
        unreachable!()
    }
    fn check_post_block(&self, _: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
        unreachable!()
    }
    fn check_unconfirmed(&self, _: &UnconfirmedReply, _: usize) -> Result<(), ReplyRefused> {
        unreachable!()
    }
    fn check_post_transactions(&self, _: &[String]) -> Result<(), ReplyRefused> {
        unreachable!()
    }
    fn throttle(&self, _: IpAddr, _: RouteId, _: Instant) -> Option<Duration> {
        unreachable!()
    }
    fn would_throttle(&self, _: IpAddr, _: RouteId, _: Instant) -> bool {
        unreachable!()
    }
    fn is_valid_peer(&self, _: &mut String, _: bool, _: &[IpAddr]) -> bool {
        unreachable!()
    }
    fn is_valid_version(&self, version: Option<&str>, _: &Params, _: &[String], _: &str) -> bool {
        version.is_some_and(|v| self.valid.iter().any(|valid| valid == v))
    }
    fn same_subnet(&self, _: IpAddr, _: IpAddr) -> bool {
        unreachable!()
    }
    fn compare_versions(&self, a: &str, b: &str) -> Option<Ordering> {
        Some(a.cmp(b))
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);

impl PeerEvents for Events {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

type Monitor = NetworkMonitor<Client, Acceptor, Rules>;

struct Net {
    monitor: Arc<Monitor>,
    client: Arc<Client>,
    acceptor: Arc<Acceptor>,
    repository: Arc<PeerRepository>,
    chain: Arc<Chain>,
    clock: Arc<ManualClock>,
    events: Arc<Events>,
}

impl Net {
    /// A peer at `ip(last)` with the state `height`, when given.
    fn add(&self, last: u8, height: Option<u32>) -> SharedPeer {
        let mut peer = Peer::new(ip(last), 4002);
        peer.state = height.map(state);
        peer.version = Some("4.3.1".into());
        let shared = SharedPeer::new(peer);
        self.repository.set(shared.clone());
        shared
    }

    fn answers(&self) -> std::sync::MutexGuard<'_, Answers> {
        self.client.answers()
    }
}

fn net_with(options: P2pOptions, sampler: Sampler, adds: bool, height: u32) -> Net {
    let repository = Arc::new(PeerRepository::new());
    let milestones = milestones();
    let last = Arc::new(block(height, Vec::new(), &milestones));
    let ranks = (0..60)
        .map(|n| (keys(100 + n).public_key().to_compressed(), n + 1))
        .collect();
    let chain = Arc::new(Chain {
        milestones,
        network: network(),
        last: Mutex::new(last),
        ranks,
        ping: Mutex::new(None),
    });
    let client = Arc::new(Client {
        answers: Mutex::default(),
        calls: Mutex::default(),
        repository: Arc::clone(&repository),
    });
    let acceptor = Arc::new(Acceptor {
        accepted: Mutex::default(),
        adds,
        repository: Arc::clone(&repository),
    });
    let clock = Arc::new(ManualClock::new(EPOCH_MS + 3_600_000));
    let events = Arc::new(Events::default());
    let monitor = Arc::new(NetworkMonitor::new(MonitorParts {
        client: Arc::clone(&client),
        acceptor: Arc::clone(&acceptor),
        rules: Arc::new(Rules {
            valid: vec!["4.3.1".into()],
        }),
        repository: Arc::clone(&repository),
        chain: Arc::clone(&chain) as Arc<dyn ChainView>,
        events: Arc::clone(&events) as Arc<dyn PeerEvents>,
        options: Arc::new(options),
        clock: Arc::clone(&clock) as Arc<dyn heartwood_kernel::Clock>,
        sampler,
        runtime: tokio::runtime::Handle::current(),
    }));
    Net {
        monitor,
        client,
        acceptor,
        repository,
        chain,
        clock,
        events,
    }
}

fn new_net() -> Net {
    let options = P2pOptions {
        minimum_network_reach: 1,
        ..P2pOptions::default()
    };
    net_with(options, Sampler::from_os().unwrap(), false, 10)
}

/// Let the spawned tasks run until they wait for a timer (a millisecond passes).
async fn settle() {
    sleep(Duration::from_millis(1)).await;
}

// ---- the network height, cleansing and pingAll -----------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_network_height_is_the_median_of_the_known_heights() {
    let net = new_net();
    assert_eq!(net.monitor.network_height(), 0);
    net.add(1, None);
    net.add(2, Some(9));
    net.add(3, Some(3));
    net.add(4, Some(5));
    net.add(5, Some(0));
    assert_eq!(net.monitor.network_height(), 5);
    net.add(6, Some(7));
    assert_eq!(net.monitor.network_height(), 7);
}

#[tokio::test(start_paused = true)]
async fn a_cleanse_races_its_ping_delay_and_gives_the_peers_still_pinging_an_infraction() {
    let net = new_net();
    let quick = net.add(1, None);
    let slow = net.add(2, None);
    net.add(3, None);
    net.answers()
        .pings
        .insert(ip(1), Ping::Ok(12, Vec::new(), None));
    net.answers()
        .pings
        .insert(ip(2), Ping::Slow(Duration::from_secs(10), 12));
    net.answers().pings.insert(ip(3), Ping::Fail);
    let started = Instant::now();
    let fast = CleanseOptions {
        fast: true,
        ..CleanseOptions::default()
    };
    net.monitor.cleanse_peers(fast).await;
    assert_eq!(started.elapsed(), Duration::from_millis(2_500));
    assert_eq!(slow.snapshot().infractions, [EPOCH_MS + 3_600_000]);
    assert!(quick.snapshot().infractions.is_empty());
    // The failed ping disconnects and forgets its peer.
    assert!(!net.repository.has(ip(3)));
    assert!(net.client.calls().contains(&Call::Removed(ip(3))));
    assert_eq!(*net.events.0.lock().unwrap(), [Event::PeerRemoved]);
    let timeouts: Vec<Duration> = (net.client.calls().iter())
        .filter_map(|call| match call {
            Call::Ping { timeout, .. } => Some(*timeout),
            _ => None,
        })
        .collect();
    assert_eq!(timeouts, [Duration::from_millis(2_500); 3]);
    // The slow ping goes on and ends later.
    advance(Duration::from_secs(8)).await;
    settle().await;
    assert_eq!(slow.snapshot().height(), Some(12));
    assert_eq!(slow.snapshot().infractions.len(), 1);

    // A full cleanse pings with verifyTimeout and returns when every ping has ended.
    let started = Instant::now();
    net.monitor.cleanse_peers(CleanseOptions::default()).await;
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    assert!(net.client.calls().contains(&Call::Ping {
        ip: ip(2),
        timeout: Duration::from_secs(60),
        force: false,
        skip: false,
    }));
}

#[tokio::test(start_paused = true)]
async fn a_cleanse_of_a_peer_count_pings_that_many() {
    let net = new_net();
    for n in 1..=6 {
        net.add(n, None);
        net.answers()
            .pings
            .insert(ip(n), Ping::Ok(12, Vec::new(), None));
    }
    let options = CleanseOptions {
        peer_count: Some(4),
        ..CleanseOptions::default()
    };
    net.monitor.cleanse_peers(options).await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Ping { .. })), 4);
    net.monitor.cleanse_peers(CleanseOptions::default()).await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Ping { .. })), 10);
}

#[tokio::test(start_paused = true)]
async fn ping_all_cleanses_fast_and_forced_at_most_every_ten_seconds() {
    let net = new_net();
    net.add(1, None);
    net.answers()
        .pings
        .insert(ip(1), Ping::Ok(12, Vec::new(), None));
    let pings = |net: &Net| net.client.count(|c| matches!(c, Call::Ping { .. }));
    net.monitor.ping_all();
    settle().await;
    assert_eq!(pings(&net), 1);
    assert_eq!(
        net.client.calls()[0],
        Call::Ping {
            ip: ip(1),
            timeout: Duration::from_millis(2_500),
            force: true,
            skip: true,
        }
    );
    net.clock.advance(10_000);
    net.monitor.ping_all();
    settle().await;
    assert_eq!(pings(&net), 1);
    net.clock.advance(1);
    net.monitor.ping_all();
    settle().await;
    assert_eq!(pings(&net), 2);
    // A finished sync pings all after 1 s.
    net.clock.advance(10_001);
    net.monitor.synced();
    settle().await;
    assert_eq!(pings(&net), 2);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(pings(&net), 3);
}

// ---- discovery and the network status --------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn discovery_asks_eight_peers_for_fifty_entries_each() {
    let net = new_net();
    for n in 1..=10 {
        net.add(n, None);
        let reply = (0..60)
            .map(|k| address(n.wrapping_mul(60).wrapping_add(k)))
            .collect();
        net.answers().peers.insert(ip(n), reply);
    }
    assert!(net.monitor.discover_peers(false, false, false).await);
    assert_eq!(net.client.count(|c| matches!(c, Call::Peers(_))), 8);
    let accepted = net.acceptor.accepted();
    // 8 replies of 50 sampled entries, merged by address (the replies overlap).
    assert!(
        accepted.len() <= 400 && accepted.len() > 50,
        "{}",
        accepted.len()
    );
    let unique: std::collections::HashSet<&str> =
        accepted.iter().map(|(a, _, _)| a.ip.as_str()).collect();
    assert_eq!(unique.len(), accepted.len());
    for (_, options, _) in &accepted {
        assert_eq!(
            *options,
            AcceptOptions {
                seed: false,
                less_verbose: true
            }
        );
    }
    // Without pinging all, half the peers' ports are probed, in the background.
    settle().await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Ports(_))), 5);
}

#[tokio::test(start_paused = true)]
async fn discovery_accepts_when_we_know_fewer_than_three_quarters_as_many_peers() {
    let net = new_net();
    for n in 1..=3 {
        net.add(n, None);
        net.answers()
            .peers
            .insert(ip(n), (1..=4).map(address).collect());
    }
    // 3 peers against 4 candidates: 3 is not fewer than 3.
    assert!(!net.monitor.discover_peers(false, false, false).await);
    assert!(net.acceptor.accepted().is_empty());
    net.answers()
        .peers
        .insert(ip(1), (1..=5).map(address).collect());
    net.answers()
        .peers
        .insert(ip(2), (1..=5).map(address).collect());
    net.answers()
        .peers
        .insert(ip(3), (1..=5).map(address).collect());
    assert!(net.monitor.discover_peers(false, false, false).await);
    assert_eq!(net.acceptor.accepted().len(), 5);
    // Pinging all accepts whatever the counts, and probes every peer's ports.
    assert!(net.monitor.discover_peers(true, false, true).await);
    settle().await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Ports(_))), 1 + 1 + 3);
}

#[tokio::test(start_paused = true)]
async fn adding_all_skips_throttled_peers_and_a_violation_skips_a_source() {
    let net = new_net();
    for n in 1..=3 {
        net.add(n, None);
        net.answers().peers.insert(ip(n), vec![address(n)]);
    }
    net.answers().throttled.push((ip(1), RouteId::GetPeers));
    let now = EPOCH_MS + 3_600_000;
    net.repository
        .get(ip(2))
        .unwrap()
        .with(|p| p.add_violation(now));
    assert!(net.monitor.discover_peers(false, true, true).await);
    assert_eq!(
        net.client
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Peers(_)))
            .cloned()
            .collect::<Vec<_>>(),
        [Call::Peers(ip(3))]
    );
    // An honest slow peer's infractions do not make it no source.
    let net = new_net();
    net.add(1, None).with(|p| {
        for t in 0..3 {
            p.add_infraction(now - t);
        }
    });
    net.answers().peers.insert(ip(1), vec![address(1)]);
    net.monitor.discover_peers(false, false, true).await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Peers(_))), 1);
}

#[tokio::test(start_paused = true)]
async fn the_network_status_runs_every_600_s_and_every_60_s_below_the_minimum() {
    let options = P2pOptions {
        minimum_network_reach: 3,
        seeds: vec![address(1)],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), true, 10);
    net.monitor.boot().await.unwrap();
    settle().await;
    // The seed, again after the first discovery found too few peers.
    let seeds = |net: &Net| {
        net.acceptor
            .accepted()
            .iter()
            .filter(|(_, o, _)| o.seed)
            .count()
    };
    assert_eq!(seeds(&net), 2);
    advance(Duration::from_secs(59)).await;
    settle().await;
    assert_eq!(seeds(&net), 2);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(seeds(&net), 3);
    // Enough peers: the next runs come every 600 s.
    net.add(2, None);
    net.add(3, None);
    advance(Duration::from_secs(60)).await;
    settle().await;
    let asked = net.client.count(|c| matches!(c, Call::Peers(_)));
    assert_eq!(seeds(&net), 3);
    advance(Duration::from_secs(599)).await;
    settle().await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Peers(_))), asked);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(net.client.count(|c| matches!(c, Call::Peers(_))) > asked);
}

#[tokio::test(start_paused = true)]
async fn without_discovery_the_network_status_runs_nothing() {
    let options = P2pOptions {
        disable_discovery: true,
        network_start: true,
        seeds: vec![address(1)],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    net.add(1, None);
    net.monitor.boot().await.unwrap();
    assert!(net.monitor.is_cold_start());
    advance(Duration::from_secs(3_600)).await;
    settle().await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Peers(_))), 0);
    net.monitor.complete_cold_start();
    assert!(!net.monitor.is_cold_start());
}

// ---- fork health ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn fork_health_counts_each_attesting_peer_once_per_active_delegate() {
    let net = net_with(
        P2pOptions::default(),
        Sampler::from_os().unwrap(),
        false,
        120,
    );
    let key = |n: u32| keys(100 + n).public_key().to_compressed();
    let forked = |common: u32| {
        Some(Verification {
            my_height: 120,
            their_height: 130,
            highest_common_height: common,
        })
    };
    // 20 active delegates on a peer forked at 9, 10 on a peer forked at 100: as text 9 is tried
    // first and wins 20 to 10, a rollback of 111 blocks.
    net.add(1, None);
    net.add(2, None);
    net.add(3, None);
    net.answers()
        .pings
        .insert(ip(1), Ping::Ok(130, (0..20).map(key).collect(), forked(9)));
    net.answers().pings.insert(
        ip(2),
        Ping::Ok(130, (20..30).map(key).collect(), forked(100)),
    );
    // Keys ranked above 53 and unknown keys do not count.
    let inactive = vec![key(55), PublicKeyBytes::from_array([2; 33])];
    net.answers()
        .pings
        .insert(ip(3), Ping::Ok(130, inactive, forked(100)));
    let status = net.monitor.check_network_health(true).await;
    assert_eq!(
        status,
        NetworkStatus {
            forked: true,
            blocks_to_rollback: Some(111)
        }
    );
    // The fork check waits 1 s first, and gives the blocks to roll back.
    let started = Instant::now();
    assert_eq!(net.monitor.check_for_fork().await, 111);
    assert!(started.elapsed() >= Duration::from_secs(1));
    // Too few attestations: no fork.
    net.answers()
        .pings
        .insert(ip(1), Ping::Ok(130, (0..10).map(key).collect(), forked(9)));
    net.answers().pings.insert(
        ip(2),
        Ping::Ok(130, (20..30).map(key).collect(), forked(100)),
    );
    assert_eq!(
        net.monitor.check_network_health(true).await,
        NetworkStatus::default()
    );
    let empty = net_with(
        P2pOptions::default(),
        Sampler::from_os().unwrap(),
        false,
        120,
    );
    assert_eq!(empty.monitor.check_for_fork().await, 0);
}

// ---- downloads -----------------------------------------------------------------------------

/// `n` peers that serve blocks up to `top`, at the network height `top`.
fn serving(n: u8, top: u32) -> Net {
    let net = new_net();
    for last in 1..=n {
        net.add(last, Some(top));
        net.answers().tops.insert(ip(last), top);
    }
    net
}

fn heights(blocks: &[DownloadedBlock]) -> Vec<u32> {
    blocks.iter().map(DownloadedBlock::height).collect()
}

async fn download(net: &Net, from: u32) -> Vec<DownloadedBlock> {
    let timeout = Duration::from_secs(30);
    net.monitor
        .download_blocks_from_height(from, 10, false, timeout, false)
        .await
}

#[tokio::test(start_paused = true)]
async fn chunk_i_is_asked_of_peer_i_and_the_chunks_are_joined() {
    let net = serving(3, 1_200);
    let blocks = download(&net, 0).await;
    assert_eq!(heights(&blocks), (1..=1_200).collect::<Vec<_>>());
    assert_eq!(
        net.client.blocks_calls(),
        [(ip(1), 0, 400), (ip(2), 400, 400), (ip(3), 800, 400)]
    );
    assert_eq!(net.monitor.chunk_size(), 400);
    let request = BlocksRequest {
        timeout: Some(Duration::from_secs(30)),
        ..BlocksRequest::after(0)
    };
    assert_eq!(net.client.calls()[0], Call::Blocks(ip(1), request));
    // The number of chunks is bounded by the network height, the peers and the parallel limit.
    let net = serving(5, 1_000);
    let limit = Duration::from_secs(2);
    let blocks = net
        .monitor
        .download_blocks_from_height(0, 2, true, limit, true)
        .await;
    assert_eq!(heights(&blocks).len(), 800);
    assert_eq!(net.client.blocks_calls().len(), 2);
    let net = serving(5, 10);
    assert_eq!(heights(&download(&net, 10).await), Vec::<u32>::new());
    assert_eq!(net.client.blocks_calls(), [(ip(1), 10, 400)]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_chunk_goes_to_the_other_peers_and_higher_chunks_are_cached() {
    let net = serving(3, 1_200);
    // Every peer fails the second chunk: the first is returned, the third cached.
    for n in 1..=3 {
        net.answers().failing.push((ip(n), 400));
    }
    let blocks = download(&net, 0).await;
    assert_eq!(heights(&blocks), (1..=400).collect::<Vec<_>>());
    let second: Vec<IpAddr> = (net.client.blocks_calls().iter())
        .filter(|(_, from, _)| *from == 400)
        .map(|(ip, _, _)| *ip)
        .collect();
    // No peer after the chunks' own: every other peer, in the repository's order.
    assert_eq!(second, [ip(2), ip(1), ip(3)]);
    assert_eq!(net.monitor.cached_chunks(), ["[801, 1,200]"]);
    // The next download from 400 asks for its last chunk as [801, ..], which the cache does not
    // hold under that key (reproduced): it is downloaded again and the cached one stays.
    net.answers().failing.clear();
    let blocks = download(&net, 400).await;
    assert_eq!(heights(&blocks), (401..=1_200).collect::<Vec<_>>());
    assert_eq!(net.monitor.cached_chunks(), ["[801, 1,200]"]);
    // With the network higher, [801, 1,200] is a middle chunk: taken from the cache, not asked.
    for n in 1..=3 {
        net.repository
            .get(ip(n))
            .unwrap()
            .with(|p| p.state = Some(state(1_600)));
        net.answers().tops.insert(ip(n), 1_600);
    }
    let before = net.client.blocks_calls().len();
    let blocks = download(&net, 400).await;
    assert_eq!(heights(&blocks), (401..=1_600).collect::<Vec<_>>());
    let asked: Vec<u32> = net.client.blocks_calls()[before..]
        .iter()
        .map(|c| c.1)
        .collect();
    assert_eq!(asked, [400, 1_200]);
    assert!(net.monitor.cached_chunks().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_chunk_size_falls_to_a_tenth_after_an_empty_download() {
    let net = serving(2, 1_000);
    net.answers().tops.clear();
    assert!(download(&net, 0).await.is_empty());
    assert_eq!(net.monitor.chunk_size(), 40);
    assert!(download(&net, 0).await.is_empty());
    assert_eq!(net.monitor.chunk_size(), 4);
    assert!(download(&net, 0).await.is_empty());
    assert_eq!(net.monitor.chunk_size(), 1);
    assert!(download(&net, 0).await.is_empty());
    assert_eq!(net.monitor.chunk_size(), 1);
    net.answers().tops.insert(ip(1), 1_000);
    net.answers().tops.insert(ip(2), 1_000);
    assert_eq!(heights(&download(&net, 0).await), [1, 2]);
    assert_eq!(net.monitor.chunk_size(), 400);
    // Chunks of 40: two peers, two chunks.
    net.answers().tops.clear();
    download(&net, 0).await;
    net.answers().tops.insert(ip(1), 1_000);
    net.answers().tops.insert(ip(2), 1_000);
    let before = net.client.blocks_calls().len();
    assert_eq!(
        heights(&download(&net, 0).await),
        (1..=80).collect::<Vec<_>>()
    );
    assert_eq!(
        net.client.blocks_calls()[before..],
        [(ip(1), 0, 40), (ip(2), 40, 40)]
    );
}

#[tokio::test(start_paused = true)]
async fn the_last_chunk_may_be_empty_but_a_reader_failure_is_not_an_answer() {
    let net = serving(3, 1_200);
    net.answers().tops.insert(ip(3), 800);
    // The last chunk's peer has nothing above 800: its empty answer is the chunk.
    net.answers().tops.insert(ip(1), 800);
    net.answers().tops.insert(ip(2), 800);
    let blocks = download(&net, 0).await;
    assert_eq!(heights(&blocks).len(), 800);
    assert_eq!(net.client.blocks_calls().len(), 3);
    // A middle chunk's empty answer is not: the other peers are asked.
    let net = serving(3, 1_200);
    net.answers().tops.insert(ip(2), 400);
    let blocks = download(&net, 0).await;
    assert_eq!(heights(&blocks).len(), 1_200);
    assert_eq!(net.client.blocks_calls().len(), 4);
    // A transaction the reader refuses fails the chunk from that peer, the last one too.
    let net = serving(2, 800);
    net.answers().failing.push((ip(2), 400));
    let blocks = download(&net, 0).await;
    assert_eq!(heights(&blocks).len(), 800);
    assert_eq!(net.client.blocks_calls()[2], (ip(1), 400, 400));
}

#[tokio::test(start_paused = true)]
async fn throttled_peers_and_peers_with_a_violation_are_no_download_source() {
    let net = serving(3, 400);
    net.answers().throttled.push((ip(1), RouteId::GetBlocks));
    let now = EPOCH_MS + 3_600_000;
    net.repository
        .get(ip(2))
        .unwrap()
        .with(|p| p.add_violation(now));
    let limit = Duration::from_secs(2);
    let blocks = net
        .monitor
        .download_blocks_from_height(0, 10, true, limit, true)
        .await;
    assert_eq!(heights(&blocks).len(), 400);
    assert_eq!(net.client.blocks_calls(), [(ip(3), 0, 400)]);
    // Without peers nothing is asked and the chunk size stays.
    let empty = new_net();
    assert!(download(&empty, 0).await.is_empty());
    assert_eq!(empty.monitor.chunk_size(), 400);
    // No source at all: no chunk, and the size falls.
    net.answers().throttled.push((ip(3), RouteId::GetBlocks));
    let blocks = net
        .monitor
        .download_blocks_from_height(0, 10, true, limit, true)
        .await;
    assert!(blocks.is_empty());
    assert_eq!(net.monitor.chunk_size(), 40);
}

#[tokio::test(start_paused = true)]
async fn a_single_block_comes_from_the_named_peer() {
    let net = serving(2, 50);
    let block = net
        .monitor
        .download_block_at_height(ip(2), 10)
        .await
        .unwrap();
    assert_eq!(block.map(|b| (b.height(), b.ip)), Some((10, ip(2))));
    let request = BlocksRequest {
        block_limit: 1,
        silent: true,
        timeout: Some(Duration::from_secs(2)),
        ..BlocksRequest::after(9)
    };
    assert_eq!(net.client.calls(), [Call::Blocks(ip(2), request)]);
    assert!(
        net.monitor
            .download_block_at_height(ip(9), 10)
            .await
            .unwrap()
            .is_none()
    );
    net.repository
        .get(ip(2))
        .unwrap()
        .with(|p| p.add_violation(EPOCH_MS + 3_600_000));
    assert!(
        net.monitor
            .download_block_at_height(ip(2), 10)
            .await
            .unwrap()
            .is_none()
    );
    net.answers().failing.push((ip(1), 9));
    assert!(
        net.monitor
            .download_block_at_height(ip(1), 10)
            .await
            .is_err()
    );
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn backup_peers_are_tried_in_the_sampled_order() {
    let options = P2pOptions {
        minimum_network_reach: 1,
        ..P2pOptions::default()
    };
    // Two chunks (the parallel limit), five peers: backups are peers 3 to 5, in the order given.
    let net = net_with(
        options,
        Sampler::scripted(vec![vec![2, 0, 1], vec![1, 2, 0]]),
        false,
        10,
    );
    for n in 1..=5 {
        net.add(n, Some(2_000));
        net.answers().tops.insert(ip(n), 2_000);
        net.answers().failing.push((ip(n), 0));
    }
    net.answers().failing.retain(|&(ip, _)| ip != self::ip(4));
    let limit = Duration::from_secs(30);
    let blocks = net
        .monitor
        .download_blocks_from_height(0, 2, false, limit, false)
        .await;
    assert_eq!(heights(&blocks).len(), 800);
    let first: Vec<IpAddr> = (net.client.blocks_calls().iter())
        .filter(|(_, from, _)| *from == 0)
        .map(|(ip, _, _)| *ip)
        .collect();
    // Peer 1, then the backups 5, 3 and 4: 4 serves.
    assert_eq!(first, [ip(1), ip(5), ip(3), ip(4)]);
}

#[tokio::test(start_paused = true)]
async fn downloaded_transactions_are_remembered_for_thirty_seconds() {
    let net = new_net();
    let params = milestones().at(1).clone();
    let [one, two, three] = [1, 2, 3].map(|n| transfer(n, &params).bytes().to_vec());
    net.add(1, None);
    net.add(2, None);
    net.answers()
        .pool
        .insert(ip(1), vec![one.clone(), two.clone(), vec![0xff]]);
    net.answers()
        .pool
        .insert(ip(2), vec![two.clone(), three.clone()]);
    let started = Instant::now();
    let mut downloaded = net.monitor.download_transactions(&[]).await;
    downloaded.sort();
    let mut expected = vec![one.clone(), two.clone(), three.clone()];
    expected.sort();
    assert_eq!(downloaded, expected);
    // A pause of a millisecond after each distinct transaction read, the unreadable one too.
    assert_eq!(started.elapsed(), Duration::from_millis(4));
    assert!(net.monitor.download_transactions(&[]).await.is_empty());
    net.clock.advance(30_000);
    assert!(net.monitor.download_transactions(&[]).await.is_empty());
    net.clock.advance(1_000);
    assert_eq!(net.monitor.download_transactions(&[]).await.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn at_most_five_unthrottled_peers_give_their_pool() {
    let net = new_net();
    for n in 1..=8 {
        net.add(n, None);
    }
    net.monitor.download_transactions(&[]).await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Unconfirmed(_))), 5);
    // Three throttled peers leave five: the throttle's answers are matched by position.
    for n in 1..=3 {
        net.answers()
            .throttled
            .push((ip(n), RouteId::GetUnconfirmedTransactions));
    }
    net.add(9, None);
    net.monitor.download_transactions(&[]).await;
    assert_eq!(net.client.count(|c| matches!(c, Call::Unconfirmed(_))), 10);
}

#[cfg(feature = "difftest")]
#[tokio::test(start_paused = true)]
async fn the_throttle_is_matched_to_the_shuffled_order_by_position() {
    let options = P2pOptions::default();
    // Peer 1 would throttle; the shuffle puts peer 3 first, and the first position is dropped.
    let net = net_with(options, Sampler::scripted(vec![vec![2, 0, 1]]), false, 10);
    for n in 1..=3 {
        net.add(n, None);
    }
    net.answers()
        .throttled
        .push((ip(1), RouteId::GetUnconfirmedTransactions));
    net.monitor.download_transactions(&[]).await;
    let asked: Vec<Call> = net.client.calls();
    assert_eq!(asked, [Call::Unconfirmed(ip(1)), Call::Unconfirmed(ip(2))]);
}

// ---- broadcasts ----------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_block_waits_for_the_rest_of_half_a_second_after_its_first_ping() {
    let net = new_net();
    net.add(1, None);
    net.add(2, None);
    let block = net.chain.last_block();
    let ping = BlockPing {
        id: *block.id(),
        height: block.height(),
        first_ms: 1_000,
        last_ms: 1_100,
        from_forger: false,
    };
    *net.chain.ping.lock().unwrap() = Some(ping.clone());
    let started = Instant::now();
    net.monitor.broadcast_block(&block).await;
    assert_eq!(started.elapsed(), Duration::from_millis(400));
    assert_eq!(
        net.client.calls(),
        [Call::PostBlock(ip(1)), Call::PostBlock(ip(2))]
    );
    // The ping moved to another height during the wait: no broadcast.
    let monitor = Arc::clone(&net.monitor);
    let sent = Arc::clone(&block);
    let waiting = tokio::spawn(async move { monitor.broadcast_block(&sent).await });
    advance(Duration::from_millis(100)).await;
    *net.chain.ping.lock().unwrap() = Some(BlockPing {
        height: block.height() + 1,
        ..ping.clone()
    });
    waiting.await.unwrap();
    assert_eq!(net.client.count(|c| matches!(c, Call::PostBlock(_))), 2);
    // A ping from the node's own forger, or 500 ms old, does not wait.
    for ping in [
        BlockPing {
            from_forger: true,
            ..ping.clone()
        },
        BlockPing {
            last_ms: 1_500,
            ..ping.clone()
        },
    ] {
        *net.chain.ping.lock().unwrap() = Some(ping);
        let started = Instant::now();
        net.monitor.broadcast_block(&block).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
    }
    assert_eq!(net.client.count(|c| matches!(c, Call::PostBlock(_))), 6);
}

#[tokio::test(start_paused = true)]
async fn transactions_are_broadcast_in_batches_of_forty_through_each_peer() {
    let net = new_net();
    net.add(1, None);
    net.add(2, None);
    let params = milestones().at(1).clone();
    let transactions: Vec<Transaction> = (1..=85).map(|n| transfer(n, &params)).collect();
    net.monitor.broadcast_transactions(&transactions);
    let batches: Vec<Call> = net.client.calls();
    assert_eq!(
        batches,
        [
            Call::PostTransactions(ip(1), 40),
            Call::PostTransactions(ip(2), 40),
            Call::PostTransactions(ip(1), 40),
            Call::PostTransactions(ip(2), 40),
            Call::PostTransactions(ip(1), 5),
            Call::PostTransactions(ip(2), 5),
        ]
    );
    net.monitor.broadcast_transactions(&[]);
    assert_eq!(net.client.calls().len(), 6);
}

// ---- seeds, sources and invalid versions ---------------------------------------------------

#[tokio::test(start_paused = true)]
async fn each_seed_is_forgotten_and_accepted_again_as_a_seed() {
    let options = P2pOptions {
        seeds: vec![address(1), address(2)],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    let known: IpAddr = address(1).ip.parse().unwrap();
    net.repository.set(SharedPeer::new(Peer::new(known, 4002)));
    net.monitor.populate_seed_peers().await.unwrap();
    let seed = AcceptOptions {
        seed: true,
        less_verbose: true,
    };
    assert_eq!(
        net.acceptor.accepted(),
        [(address(1), seed, false), (address(2), seed, false)]
    );
    let none = net_with(
        P2pOptions::default(),
        Sampler::from_os().unwrap(),
        false,
        10,
    );
    assert_eq!(
        none.monitor.populate_seed_peers().await,
        Err(SeedError::NoSeeds)
    );
    assert_eq!(none.monitor.boot().await, Err(SeedError::NoSeeds));
}

#[tokio::test]
async fn a_local_file_source_grows_the_seeds_for_good() {
    let dir = std::env::temp_dir().join(format!("heartwood-p2p-sources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("peers.json");
    let write = |peers: serde_json::Value| std::fs::write(&file, peers.to_string()).unwrap();
    write(json!([{"ip": "198.51.100.1", "port": 4000}, {"ip": "198.51.100.3", "port": 4003}]));
    let options = P2pOptions {
        seeds: vec![address(1)],
        sources: vec![
            file.to_string_lossy().into_owned(),
            "http://192.0.2.1/".into(),
        ],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    net.monitor.populate_seed_peers().await.unwrap();
    let seeds: Vec<PeerAddress> = net.acceptor.accepted().into_iter().map(|a| a.0).collect();
    assert_eq!(
        seeds,
        [
            address(1),
            PeerAddress {
                ip: "198.51.100.3".into(),
                port: 4003
            }
        ]
    );
    // The source changes: the seeds it gave stay.
    write(json!([{"ip": "198.51.100.4", "port": 4004}]));
    net.monitor.populate_seed_peers().await.unwrap();
    assert_eq!(net.acceptor.accepted().len(), 2 + 3);
    // An unreadable source adds nothing.
    std::fs::remove_file(&file).unwrap();
    net.monitor.populate_seed_peers().await.unwrap();
    assert_eq!(net.acceptor.accepted().len(), 2 + 3 + 3);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A local HTTP server that answers every connection with `response`, and counts them.
async fn http_server(response: &'static str) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buffer = [0u8; 4096];
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buffer[..read]).into_owned());
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (port, requests)
}

#[tokio::test]
async fn a_plain_http_source_is_fetched_and_an_https_one_refused() {
    let body = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[{\"ip\":\"198.51.100.9\",\"port\":4009}]";
    let (port, requests) = http_server(body).await;
    let options = P2pOptions {
        seeds: vec![address(1)],
        sources: vec![format!("http://127.0.0.1:{port}/peers.json")],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    net.monitor.populate_seed_peers().await.unwrap();
    assert_eq!(net.acceptor.accepted().len(), 2);
    assert!(requests.lock().unwrap()[0].starts_with("GET /peers.json HTTP/1.0\r\n"));
    // An error status gives no peers.
    let (port, _) = http_server("HTTP/1.1 404 Not Found\r\n\r\n[]").await;
    let options = P2pOptions {
        seeds: vec![address(1)],
        sources: vec![format!("http://127.0.0.1:{port}/")],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    net.monitor.populate_seed_peers().await.unwrap();
    assert_eq!(net.acceptor.accepted().len(), 1);
    // HTTPS refuses the start, naming the source.
    let options = P2pOptions {
        seeds: vec![address(1)],
        sources: vec!["https://192.0.2.1/peers.json".into()],
        ..P2pOptions::default()
    };
    let net = net_with(options, Sampler::from_os().unwrap(), false, 10);
    let refused = net.monitor.boot().await;
    assert!(
        matches!(refused, Err(SeedError::Source { source_name, .. }) if source_name.starts_with("https://"))
    );
    assert!(net.acceptor.accepted().is_empty());
}

#[tokio::test]
async fn plugin_ports_are_probed_with_an_http_head() {
    let (ok, requests) = http_server("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
    let (missing, _) = http_server("HTTP/1.1 404 Not Found\r\n\r\n").await;
    let probe = HttpProbe::default();
    let local = IpAddr::from([127, 0, 0, 1]);
    assert_eq!(probe.head(local, ok).await, Some(200));
    assert!(requests.lock().unwrap()[0].starts_with("HEAD / HTTP/1.0\r\n"));
    assert_eq!(probe.head(local, missing).await, Some(404));
    // Nothing listens: no answer.
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(probe.head(local, closed).await, None);
    // A server that never answers: no answer within the timeout.
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = silent.local_addr().unwrap().port();
    let quick = HttpProbe {
        timeout: Duration::from_millis(200),
    };
    assert_eq!(quick.head(local, port).await, None);
    // The reference's probe URL of an IPv6 address names another host: never an answer.
    assert_eq!(probe.head("::1".parse().unwrap(), ok).await, None);
    drop(silent);
}

#[tokio::test(start_paused = true)]
async fn peers_whose_version_no_longer_passes_are_disconnected() {
    let net = new_net();
    net.add(1, None);
    net.add(2, None).with(|p| p.version = Some("3.0.0".into()));
    net.add(3, None).with(|p| p.version = None);
    net.monitor.disconnect_invalid_peers();
    assert_eq!(
        net.client.calls(),
        [Call::Removed(ip(2)), Call::Removed(ip(3))]
    );
    assert_eq!(net.repository.len(), 1);
    let listed = net.monitor.peers();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].ip, listed[0].port), (ip(1), 4002));
    assert_eq!(listed[0].version.as_deref(), Some("4.3.1"));
}

// ---- a downloaded block rebuilt --------------------------------------------------------------

#[test]
fn a_downloaded_block_is_rebuilt_with_the_ids_of_its_canonical_bytes() {
    let milestones = milestones();
    let params = milestones.at(3).clone();
    let made = block(
        3,
        vec![transfer(1, &params), transfer(2, &params)],
        &milestones,
    );
    let from = ip(7);
    let downloaded = read_listed(listed(&made), from, false, &params).unwrap();
    assert_eq!(downloaded.ip, from);
    assert!(downloaded.header.transactions.is_empty());
    let rebuilt = to_block(&downloaded, &milestones).unwrap();
    assert_eq!(rebuilt.id(), made.id());
    assert_eq!(rebuilt.encode(), made.encode());
    // With headers only, the block is its header, with the same id.
    let header_only = read_listed(listed(&made), from, true, &params).unwrap();
    assert!(header_only.transactions.is_none());
    let rebuilt = to_block(&header_only, &milestones).unwrap();
    assert_eq!(rebuilt.id(), made.id());
    assert!(rebuilt.transactions().is_empty());
}

#[test]
fn the_header_is_written_as_the_reference_writes_it() {
    let milestones = milestones();
    let params = milestones.at(3).clone();
    let made = block(3, vec![transfer(1, &params)], &milestones);
    let rebuild = |change: &dyn Fn(&mut ListedBlock)| {
        let mut block = listed(&made);
        change(&mut block);
        let downloaded = read_listed(block, ip(1), false, &params).unwrap();
        to_block(&downloaded, &milestones).map(|b| *b.id())
    };
    // An odd last hex digit is dropped, and absent counts are the defaults.
    let odd = |b: &mut ListedBlock| b.header.payload_hash.as_mut().unwrap().push('7');
    assert_eq!(rebuild(&odd), Ok(*made.id()));
    let absent = |b: &mut ListedBlock| {
        b.header.version = None;
        b.header.number_of_transactions = None;
    };
    assert_eq!(rebuild(&absent), Ok(*made.id()));
    let refused = |field| Err(AssembleError::Field { field });
    let short = |b: &mut ListedBlock| {
        b.header.payload_hash.as_mut().unwrap().pop();
        b.header.payload_hash.as_mut().unwrap().pop();
    };
    assert_eq!(rebuild(&short), refused("payloadHash"));
    let no_hash = |b: &mut ListedBlock| b.header.payload_hash = None;
    assert_eq!(rebuild(&no_hash), refused("payloadHash"));
    let miscounted = |b: &mut ListedBlock| b.header.number_of_transactions = Some(2);
    assert_eq!(rebuild(&miscounted), refused("numberOfTransactions"));
    let zero = |b: &mut ListedBlock| b.header.number_of_transactions = Some(0);
    assert_eq!(rebuild(&zero), refused("numberOfTransactions"));
    let negative = |b: &mut ListedBlock| b.total_fee = Amount::new(-1);
    assert_eq!(rebuild(&negative), refused("totalFee"));
    // At height 1 a downloaded block is received, not read under the genesis rules: a
    // transaction with no fee refuses it.
    let genesis = milestones.at(1).clone();
    let free = TransactionBuilder::transfer(vec![TransferItem {
        amount: 1,
        recipient: Address::from_public_key(keys(0).public_key(), 90),
    }])
    .nonce(1)
    .fee(0)
    .build_with(&keys(1), Aux::random(), &genesis, Mode::Genesis)
    .expect("a genesis transfer");
    let first = block(1, vec![free], &milestones);
    let downloaded = read_listed(listed(&first), ip(1), false, &genesis).unwrap();
    assert!(matches!(
        to_block(&downloaded, &milestones),
        Err(AssembleError::Block(_))
    ));
    let without = block(1, Vec::new(), &milestones);
    let downloaded = read_listed(listed(&without), ip(1), false, &genesis).unwrap();
    assert_eq!(
        to_block(&downloaded, &milestones).map(|b| *b.id()),
        Ok(*without.id())
    );
    // A transaction the reader refuses fails the download.
    let mut garbage = listed(&made);
    garbage.transactions.push(vec![0xff]);
    let failed = read_listed(garbage, ip(1), false, &params);
    assert!(matches!(
        failed,
        Err(DownloadError::Transaction { height: 3, .. })
    ));
}

// ---- with the real client ------------------------------------------------------------------

/// A cleanse and a download through the communicator, the connector and the verifier against
/// three mock peers that serve the same chain, 6 blocks ahead of ours.
#[cfg(feature = "difftest")]
mod with_the_real_client {
    use std::net::SocketAddr;

    use heartwood_crypto::BlockHeader;
    use heartwood_kernel::{RoundInfo, SystemClock};
    use heartwood_p2p::errors::LookupError;
    use heartwood_p2p::testing::{MockPeer, ServedChain};
    use heartwood_p2p::utils::rules::Rules as RealRules;
    use heartwood_p2p::view::{BlockLookup, OnlineSink};
    use heartwood_p2p::{Communicator, CommunicatorParts, Connector, PeerVerifier};
    use heartwood_p2p_wire::status::{NetworkConfig, PeerConfig, TokenConfig};

    use super::*;

    /// A chain of `n` blocks from height 1, each chained to the one before.
    fn chain_of(n: u32, milestones: &Milestones) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        for height in 1..=n {
            let previous = blocks.last().map_or([0; 32], |b| {
                hex::decode_array::<32>(&b.id().to_hex()).expect("an id")
            });
            let draft = BlockDraft {
                version: 0,
                timestamp: 8 * (height - 1),
                height,
                previous_block: previous,
                number_of_transactions: 0,
                total_amount: 0,
                total_fee: 0,
                reward: 0,
                payload_length: 0,
                payload_hash: heartwood_crypto::crypto::hash::sha256(&[]),
            };
            let made = factory::make(draft, Vec::new(), &keys(2), Aux::random(), milestones);
            blocks.push(made.expect("a block"));
        }
        blocks
    }

    /// Our chain: its first `ours` blocks, and the generator as the only delegate.
    struct Ours {
        blocks: Vec<Arc<Block>>,
        milestones: Milestones,
        network: Network,
    }

    impl ChainView for Ours {
        fn last_block(&self) -> Arc<Block> {
            Arc::clone(self.blocks.last().expect("a block"))
        }
        fn last_blocks(&self) -> Vec<Arc<Block>> {
            self.blocks.clone()
        }
        fn milestones(&self) -> &Milestones {
            &self.milestones
        }
        fn network(&self) -> &Network {
            &self.network
        }
        fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<DelegateInfo> {
            (*key == keys(2).public_key().to_compressed()).then(|| DelegateInfo {
                username: "generator".into(),
                rank: Some(1),
            })
        }
        fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
            vec![keys(2).public_key().to_compressed()]
        }
        fn is_synced(&self) -> bool {
            false
        }
        fn is_idle(&self) -> bool {
            false
        }
        fn block_ping(&self) -> Option<BlockPing> {
            None
        }
    }

    impl BlockLookup for Ours {
        fn blocks_by_height(&self, heights: &[u32]) -> Result<Vec<BlockHeader>, LookupError> {
            let at = |height: &u32| self.blocks.iter().find(|b| b.height() == *height);
            Ok(heights
                .iter()
                .filter_map(at)
                .map(|b| b.header().clone())
                .collect())
        }
        fn round_delegate_keys(&self, _: RoundInfo) -> Result<Vec<PublicKeyBytes>, LookupError> {
            Ok(self.delegate_keys())
        }
    }

    impl OnlineSink for Ours {
        fn set_version(&self, _: PublicKeyBytes, _: u64, _: String) {}
    }

    #[tokio::test]
    #[ignore = "integration"]
    async fn a_cleanse_and_a_download_against_three_mock_peers() {
        let milestones = milestones();
        let served = chain_of(12, &milestones);
        let config = PeerConfig {
            version: "4.3.1".into(),
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
        let mut mocks = Vec::new();
        for last in 2..=4 {
            let chain = ServedChain {
                blocks: served.clone(),
                milestones: milestones.clone(),
                config: config.clone(),
                signers: Vec::new(),
                peers: Vec::new(),
                empty_first_get_blocks: false,
            };
            let at = IpAddr::from([127, 0, 0, last]);
            mocks.push(MockPeer::serving(at, chain).await.expect("a mock peer"));
        }
        let ours = Arc::new(Ours {
            blocks: served.iter().take(6).cloned().map(Arc::new).collect(),
            milestones: milestones.clone(),
            network: network(),
        });
        let options = Arc::new(P2pOptions {
            version: "4.3.1-heartwood.0".into(),
            minimum_versions: vec![">=4.3.0".into()],
            minimum_network_reach: 1,
            ..P2pOptions::default()
        });
        let repository = Arc::new(PeerRepository::new());
        let addresses: Vec<SocketAddr> = mocks.iter().map(MockPeer::address).collect();
        for address in &addresses {
            repository.set(SharedPeer::new(Peer::new(address.ip(), address.port())));
        }
        let clock: Arc<dyn heartwood_kernel::Clock> = Arc::new(SystemClock);
        let rules = Arc::new(RealRules::new(100, 25));
        let events: Arc<dyn PeerEvents> = Arc::new(Events::default());
        let chain: Arc<dyn ChainView> = ours.clone();
        let verifier = PeerVerifier::new(Arc::clone(&chain), ours.clone(), Arc::clone(&clock));
        let client = Arc::new(
            Communicator::new(CommunicatorParts {
                transport: Arc::new(Connector::new()),
                verifier: Arc::new(verifier),
                rules: Arc::clone(&rules),
                probe: Arc::new(HttpProbe::default()),
                repository: Arc::clone(&repository),
                chain: Arc::clone(&chain),
                online: ours.clone(),
                events: Arc::clone(&events),
                options: Arc::clone(&options),
                clock: Arc::clone(&clock),
                runtime: tokio::runtime::Handle::current(),
            })
            .unwrap(),
        );
        let acceptor = Arc::new(Acceptor {
            accepted: Mutex::default(),
            adds: false,
            repository: Arc::clone(&repository),
        });
        let monitor = Arc::new(NetworkMonitor::new(MonitorParts {
            client,
            acceptor,
            rules,
            repository: Arc::clone(&repository),
            chain,
            events,
            options,
            clock,
            sampler: Sampler::from_os().unwrap(),
            runtime: tokio::runtime::Handle::current(),
        }));
        let forced = CleanseOptions {
            fast: true,
            force_ping: true,
            ..CleanseOptions::default()
        };
        monitor.cleanse_peers(forced).await;
        for address in &addresses {
            let peer = repository
                .get(address.ip())
                .expect("the peer stays")
                .snapshot();
            assert_eq!(peer.height(), Some(12), "{address}");
            assert!(peer.infractions.is_empty() && peer.violations.is_empty());
            assert_eq!(peer.verification.map(|v| v.highest_common_height), Some(6));
        }
        assert_eq!(monitor.network_height(), 12);
        let timeout = Duration::from_secs(30);
        let blocks = monitor
            .download_blocks_from_height(6, 10, false, timeout, false)
            .await;
        assert_eq!(heights(&blocks), (7..=12).collect::<Vec<_>>());
        for (downloaded, expected) in blocks.iter().zip(served.iter().skip(6)) {
            assert_eq!(downloaded.ip, addresses[0].ip());
            assert_eq!(
                to_block(downloaded, &milestones).unwrap().id(),
                expected.id()
            );
        }
        assert_eq!(monitor.chunk_size(), 400);
    }
}
