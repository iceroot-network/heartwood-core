//! Fakes of the client's neighbours for the tests of the communicator, the acceptance and the
//! protocol violations: a scripted transport, scripted peer rules, a verifier, a port probe, the
//! node's chain, and recorders of the online record and the peer events. Each test file uses
//! part of them.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::{Address, Aux, Block, KeyPair, Milestones, Network, Params, PublicKeyBytes};
use heartwood_kernel::{Event, ManualClock};
use heartwood_p2p::errors::VerificationStopped;
use heartwood_p2p::seams::{
    Deadline, PeerRules, PeerTransport, PortProbe, StateCheck, VerifierLink,
};
use heartwood_p2p::view::{BlockPing, DelegateInfo};
use heartwood_p2p::{
    ChainView, Communicator, CommunicatorParts, FastVerification, OnlineSink, P2pOptions,
    PeerEvents, PeerRepository, PeerState, SessionError, SharedPeer, Verification,
};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::ListedBlock;
use heartwood_p2p_wire::errors::{ReplyRefused, RequestError};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use heartwood_p2p_wire::status::{
    self, NetworkConfig, PeerConfig, PluginConfig, SignedState, StatusHeaderData, TokenConfig,
};
use serde_json::Value;
use tokio::time::Instant;

/// The devnet network and milestones of the crypto crate's committed vectors.
pub struct Devnet {
    pub network: Network,
    pub milestones: Milestones,
    pub milestones_text: String,
}

pub static DEVNET: LazyLock<Devnet> = LazyLock::new(|| {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crypto/tests/vectors/V10-genesis.jsonl"
    );
    let text = std::fs::read_to_string(path).expect("the crypto crate's V10 vectors");
    let record: Value = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
        .find(|record| record["op"] != "meta")
        .expect("a chain definition");
    let files = &record["output"]["files"];
    let network = Network::load(files["crypto/network.json"].as_str().unwrap()).unwrap();
    let milestones_text = files["crypto/milestones.json"].as_str().unwrap().to_owned();
    let milestones = Milestones::load(&milestones_text, &network).unwrap();
    Devnet {
        network,
        milestones,
        milestones_text,
    }
});

/// The devnet milestones with one more milestone at `height` that changes `block.maxTransactions`.
pub fn milestones_with_max_transactions(height: u32, max_transactions: u32) -> Milestones {
    let mut list: Vec<Value> = serde_json::from_str(&DEVNET.milestones_text).unwrap();
    list.push(serde_json::json!({
        "height": height,
        "block": {"version": 0, "maxTransactions": max_transactions, "maxPayload": 2_097_152}
    }));
    Milestones::load(&serde_json::to_string(&list).unwrap(), &DEVNET.network).unwrap()
}

/// The keys of delegate `index`.
pub fn delegate(index: usize) -> KeyPair {
    KeyPair::from_passphrase(&format!("peer test delegate {index}")).unwrap()
}

/// A block at `height` forged by delegate 0.
pub fn block(height: u32) -> Block {
    let draft = BlockDraft {
        version: 0,
        timestamp: height.saturating_mul(8),
        height,
        previous_block: [height.to_le_bytes()[0]; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: [0; 32],
    };
    factory::make(
        draft,
        Vec::new(),
        &delegate(0),
        Aux::random(),
        &DEVNET.milestones,
    )
    .unwrap()
}

/// A transfer signed by delegate 1, as bytes.
pub fn transfer_bytes(nonce: u64) -> Vec<u8> {
    use heartwood_crypto::transactions::builders::TransactionBuilder;
    use heartwood_crypto::transactions::types::TransferItem;
    let params = DEVNET.milestones.at(1);
    let recipient = Address::from_public_key(delegate(2).public_key(), DEVNET.network.pub_key_hash);
    TransactionBuilder::transfer(vec![TransferItem {
        amount: 1,
        recipient,
    }])
    .network(DEVNET.network.pub_key_hash)
    .nonce(nonce)
    .fee(50_000_000)
    .build(&delegate(1), Aux::random(), params)
    .unwrap()
    .bytes()
    .to_vec()
}

/// A transfer signed by delegate 1 whose memo is not UTF-8, as bytes. Its signature no longer
/// holds, which the reader of downloaded blocks does not check.
pub fn transfer_with_non_utf8_memo() -> Vec<u8> {
    use heartwood_crypto::transactions::builders::TransactionBuilder;
    use heartwood_crypto::transactions::types::{Memo, TransferItem};
    let params = DEVNET.milestones.at(1);
    let recipient = Address::from_public_key(delegate(2).public_key(), DEVNET.network.pub_key_hash);
    let transfer = TransactionBuilder::transfer(vec![TransferItem {
        amount: 1,
        recipient,
    }])
    .network(DEVNET.network.pub_key_hash)
    .nonce(1)
    .fee(50_000_000)
    .memo(Memo::new("memo!").unwrap())
    .build(&delegate(1), Aux::random(), params)
    .unwrap();
    let mut bytes = transfer.bytes().to_vec();
    let at = bytes.windows(6).position(|w| w == b"\x05memo!").unwrap();
    bytes[at + 1] = 0xff;
    bytes
}

// ---- the node's chain ----------------------------------------------------------------------

/// The node's chain: a last block, the delegates by key, and the machine's state.
pub struct Chain {
    pub last: Mutex<Arc<Block>>,
    pub milestones: Milestones,
    pub network: Network,
    /// The registered delegates, in `allByUsername` order.
    pub delegates: Vec<PublicKeyBytes>,
    pub idle: AtomicBool,
}

impl Chain {
    pub fn at(height: u32) -> Chain {
        Chain::with(height, DEVNET.milestones.clone())
    }

    pub fn with(height: u32, milestones: Milestones) -> Chain {
        Chain {
            last: Mutex::new(Arc::new(block(height))),
            milestones,
            network: DEVNET.network.clone(),
            delegates: (0..5)
                .map(|i| delegate(i).public_key().to_compressed())
                .collect(),
            idle: AtomicBool::new(false),
        }
    }

    pub fn last_id(&self) -> String {
        self.last.lock().unwrap().id().to_hex()
    }
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
        let rank = self.delegates.iter().position(|known| known == key)?;
        Some(DelegateInfo {
            username: format!("delegate_{rank}"),
            rank: u32::try_from(rank + 1).ok(),
        })
    }

    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.delegates.clone()
    }

    fn is_synced(&self) -> bool {
        true
    }

    fn is_idle(&self) -> bool {
        self.idle.load(Ordering::SeqCst)
    }

    fn block_ping(&self) -> Option<BlockPing> {
        None
    }
}

// ---- the transport ---------------------------------------------------------------------------

/// What the fake transport answers to one request.
#[derive(Debug, Clone)]
pub enum Answer {
    /// These bytes.
    Bytes(Vec<u8>),
    /// This error.
    Fail(SessionError),
    /// Wait, then the inner answer.
    After(Duration, Box<Answer>),
}

/// A request the fake transport received.
#[derive(Debug, Clone)]
pub struct Sent {
    pub address: SocketAddr,
    pub route: RouteId,
    pub payload: Vec<u8>,
    pub timeout: Duration,
    pub limit: usize,
    pub at: Instant,
}

/// A transport that answers each route from a script, in order; a route with no answer left
/// times out after the request's timeout.
#[derive(Default)]
pub struct Transport {
    answers: Mutex<HashMap<RouteId, VecDeque<Answer>>>,
    pub sent: Mutex<Vec<Sent>>,
    pub disconnected: Mutex<Vec<IpAddr>>,
    pub errors: Mutex<Vec<(IpAddr, &'static str)>>,
    pub forgotten: AtomicUsize,
}

impl Transport {
    pub fn answer(&self, route: RouteId, answer: Answer) {
        self.answers
            .lock()
            .unwrap()
            .entry(route)
            .or_default()
            .push_back(answer);
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    pub fn sent_on(&self, route: RouteId) -> Vec<Sent> {
        self.sent()
            .into_iter()
            .filter(|sent| sent.route == route)
            .collect()
    }

    pub fn disconnected(&self) -> Vec<IpAddr> {
        self.disconnected.lock().unwrap().clone()
    }

    pub fn errors(&self) -> Vec<(IpAddr, &'static str)> {
        self.errors.lock().unwrap().clone()
    }
}

impl PeerTransport for Transport {
    async fn emit(
        &self,
        address: SocketAddr,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        self.sent.lock().unwrap().push(Sent {
            address,
            route,
            payload,
            timeout,
            limit: max_payload,
            at: Instant::now(),
        });
        let answer = self
            .answers
            .lock()
            .unwrap()
            .get_mut(&route)
            .and_then(VecDeque::pop_front);
        let mut answer = answer.unwrap_or(Answer::After(
            timeout,
            Box::new(Answer::Fail(SessionError::Timeout)),
        ));
        loop {
            match answer {
                Answer::Bytes(bytes) => return Ok(bytes),
                Answer::Fail(error) => return Err(error),
                Answer::After(wait, inner) => {
                    tokio::time::sleep(wait).await;
                    answer = *inner;
                }
            }
        }
    }

    fn disconnect(&self, ip: IpAddr) {
        self.disconnected.lock().unwrap().push(ip);
    }

    fn set_error(&self, ip: IpAddr, name: &'static str) {
        self.errors.lock().unwrap().push((ip, name));
    }

    fn forget_error(&self, _ip: IpAddr) {
        self.forgotten.fetch_add(1, Ordering::SeqCst);
    }
}

// ---- the peer rules --------------------------------------------------------------------------

/// Peer rules whose reply checks accept everything except the routes set to refuse, whose throttle
/// asks for the scripted waits, and whose address and version rules are simple stand-ins.
#[derive(Default)]
pub struct Rules {
    pub encoded: Mutex<Vec<ClientRequest>>,
    pub waits: Mutex<VecDeque<Duration>>,
    pub refuse: Mutex<HashSet<RouteId>>,
    pub throttled: AtomicBool,
    /// Versions the gate refuses.
    pub bad_versions: Mutex<HashSet<String>>,
}

impl Rules {
    pub fn refuse(&self, route: RouteId) {
        self.refuse.lock().unwrap().insert(route);
    }

    fn check(&self, route: RouteId) -> Result<(), ReplyRefused> {
        if self.refuse.lock().unwrap().contains(&route) {
            Err(ReplyRefused::Root)
        } else {
            Ok(())
        }
    }
}

impl PeerRules for Rules {
    fn encode_request(&self, request: &ClientRequest, _headers: &Headers) -> Vec<u8> {
        self.encoded.lock().unwrap().push(request.clone());
        request.route().id().as_bytes().to_vec()
    }

    fn check_peers(&self, peers: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
        self.check(RouteId::GetPeers)?;
        Ok(peers
            .iter()
            .map(|peer| PeerAddress {
                ip: peer.ip.clone().unwrap_or_default(),
                port: u16::try_from(peer.port.unwrap_or(0)).unwrap(),
            })
            .collect())
    }

    fn check_common_blocks(
        &self,
        reply: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused> {
        self.check(RouteId::GetCommonBlocks)?;
        Ok(reply.common.as_ref().map(|common| CommonBlock {
            height: common.height.unwrap_or(0),
            id: common.id.clone().unwrap_or_default(),
        }))
    }

    fn check_blocks(&self, _blocks: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
        self.check(RouteId::GetBlocks)
    }

    fn check_post_block(&self, reply: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
        self.check(RouteId::PostBlock)?;
        Ok(reply.height)
    }

    fn check_unconfirmed(
        &self,
        reply: &UnconfirmedReply,
        max_transactions: usize,
    ) -> Result<(), ReplyRefused> {
        self.check(RouteId::GetUnconfirmedTransactions)?;
        if reply.transactions.len() > max_transactions {
            return Err(ReplyRefused::Unconfirmed);
        }
        Ok(())
    }

    fn check_post_transactions(&self, _accept: &[String]) -> Result<(), ReplyRefused> {
        self.check(RouteId::PostTransactions)
    }

    fn throttle(&self, _ip: IpAddr, _route: RouteId, _now: Instant) -> Option<Duration> {
        self.waits.lock().unwrap().pop_front()
    }

    fn would_throttle(&self, _ip: IpAddr, _route: RouteId, _now: Instant) -> bool {
        self.throttled.load(Ordering::SeqCst)
    }

    /// A stand-in for the address rules: an IPv4-mapped IPv6 address becomes IPv4; loopback,
    /// unspecified and (without `allow_private`) private addresses, text starting with `0`, and
    /// the host's addresses are refused.
    fn is_valid_peer(&self, ip: &mut String, allow_private: bool, interfaces: &[IpAddr]) -> bool {
        let Ok(parsed) = ip.parse::<IpAddr>() else {
            return false;
        };
        let parsed = match parsed {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
            v4 => v4,
        };
        *ip = parsed.to_string();
        let private = matches!(parsed, IpAddr::V4(v4) if v4.is_private());
        !ip.starts_with('0')
            && !parsed.is_loopback()
            && !parsed.is_unspecified()
            && (allow_private || !private)
            && !interfaces.contains(&parsed)
    }

    fn is_valid_version(
        &self,
        version: Option<&str>,
        _milestone: &Params,
        _configured: &[String],
        _network_name: &str,
    ) -> bool {
        version.is_some_and(|version| !self.bad_versions.lock().unwrap().contains(version))
    }

    fn same_subnet(&self, a: IpAddr, b: IpAddr) -> bool {
        match (a, b) {
            (IpAddr::V4(a), IpAddr::V4(b)) => a.octets()[..3] == b.octets()[..3],
            _ => false,
        }
    }

    fn compare_versions(&self, _a: &str, _b: &str) -> Option<std::cmp::Ordering> {
        None
    }
}

// ---- the verifier and the probe ---------------------------------------------------------------

/// A verifier that answers what it was set to, after its delay.
pub struct Verifier {
    pub full: Mutex<Result<Option<Verification>, VerificationStopped>>,
    pub fast: Mutex<Result<Option<FastVerification>, VerificationStopped>>,
    pub delay: Mutex<Duration>,
    pub claimed: Mutex<Vec<PeerState>>,
    pub deadlines: Mutex<Vec<Deadline>>,
}

impl Default for Verifier {
    fn default() -> Verifier {
        Verifier {
            full: Mutex::new(Ok(Some(Verification {
                my_height: 10,
                their_height: 10,
                highest_common_height: 10,
            }))),
            fast: Mutex::new(Ok(Some(FastVerification {
                my_height: 10,
                their_height: 10,
                highest_common_height: Some(10),
            }))),
            delay: Mutex::new(Duration::ZERO),
            claimed: Mutex::default(),
            deadlines: Mutex::default(),
        }
    }
}

impl StateCheck for Verifier {
    async fn check_state<L: VerifierLink>(
        &self,
        _link: &L,
        _peer: &SharedPeer,
        claimed: &PeerState,
        deadline: Deadline,
    ) -> Result<Option<Verification>, VerificationStopped> {
        self.claimed.lock().unwrap().push(claimed.clone());
        self.deadlines.lock().unwrap().push(deadline);
        let delay = *self.delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        *self.full.lock().unwrap()
    }

    async fn check_state_fast(
        &self,
        _peer: &SharedPeer,
        claimed: &PeerState,
    ) -> Result<Option<FastVerification>, VerificationStopped> {
        self.claimed.lock().unwrap().push(claimed.clone());
        *self.fast.lock().unwrap()
    }
}

/// A port probe for which the ports in `open` answer 200 and the others 404.
#[derive(Default)]
pub struct Probe {
    pub open: Mutex<HashSet<u16>>,
    pub probed: Mutex<Vec<(IpAddr, u16)>>,
}

impl PortProbe for Probe {
    async fn head(&self, ip: IpAddr, port: u16) -> Option<u16> {
        self.probed.lock().unwrap().push((ip, port));
        Some(if self.open.lock().unwrap().contains(&port) {
            200
        } else {
            404
        })
    }
}

// ---- recorders -------------------------------------------------------------------------------

/// The online record's writes.
#[derive(Default)]
pub struct Online(pub Mutex<Vec<(PublicKeyBytes, u64, String)>>);

impl OnlineSink for Online {
    fn set_version(&self, key: PublicKeyBytes, round: u64, version: String) {
        self.0.lock().unwrap().push((key, round, version));
    }
}

/// The peer events.
#[derive(Default)]
pub struct Events(pub Mutex<Vec<Event>>);

impl Events {
    pub fn all(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }
}

impl PeerEvents for Events {
    fn emit(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

// ---- a communicator over the fakes ------------------------------------------------------------

/// The devnet's epoch, in milliseconds since the Unix epoch.
pub const EPOCH_MS: i64 = 1_767_225_600_000;
/// The time the manual clock starts at, in milliseconds since the Unix epoch: the devnet epoch
/// plus 1,000 s (slot 125).
pub const START_MS: i64 = EPOCH_MS + 1_000_000;

/// The options of the tests: version 4.3.1, the reference's defaults otherwise.
pub fn options() -> P2pOptions {
    P2pOptions {
        version: "4.3.1".to_owned(),
        ..P2pOptions::default()
    }
}

/// A communicator over the fakes, with everything it was made of.
pub struct Fixture<Tr = Transport, Ru = Rules> {
    pub transport: Arc<Tr>,
    pub verifier: Arc<Verifier>,
    pub rules: Arc<Ru>,
    pub probe: Arc<Probe>,
    pub repository: Arc<PeerRepository>,
    pub chain: Arc<Chain>,
    pub online: Arc<Online>,
    pub events: Arc<Events>,
    pub clock: Arc<ManualClock>,
    pub options: Arc<P2pOptions>,
    pub communicator: Communicator<Tr, Verifier, Ru, Probe>,
}

impl Fixture {
    pub fn new() -> Fixture {
        Fixture::with(Chain::at(10), options())
    }

    pub fn with(chain: Chain, options: P2pOptions) -> Fixture {
        Fixture::over(Transport::default(), Rules::default(), chain, options)
    }
}

impl<Tr, Ru> Fixture<Tr, Ru>
where
    Tr: PeerTransport + 'static,
    Ru: PeerRules + 'static,
{
    pub fn over(transport: Tr, rules: Ru, chain: Chain, options: P2pOptions) -> Fixture<Tr, Ru> {
        Fixture::try_over(transport, rules, chain, options).expect("the options name a version")
    }

    pub fn try_over(
        transport: Tr,
        rules: Ru,
        chain: Chain,
        options: P2pOptions,
    ) -> Result<Fixture<Tr, Ru>, RequestError> {
        let (transport, rules) = (Arc::new(transport), Arc::new(rules));
        let verifier = Arc::new(Verifier::default());
        let probe = Arc::new(Probe::default());
        let repository = Arc::new(PeerRepository::new());
        let chain = Arc::new(chain);
        let online = Arc::new(Online::default());
        let events = Arc::new(Events::default());
        let clock = Arc::new(ManualClock::new(START_MS));
        let options = Arc::new(options);
        let communicator = Communicator::new(CommunicatorParts {
            transport: Arc::clone(&transport),
            verifier: Arc::clone(&verifier),
            rules: Arc::clone(&rules),
            probe: Arc::clone(&probe),
            repository: Arc::clone(&repository),
            chain: chain.clone(),
            online: online.clone(),
            events: events.clone(),
            options: Arc::clone(&options),
            clock: clock.clone(),
            runtime: tokio::runtime::Handle::current(),
        })?;
        Ok(Fixture {
            transport,
            verifier,
            rules,
            probe,
            repository,
            chain,
            online,
            events,
            clock,
            options,
            communicator,
        })
    }
}

/// A peer at 192.0.2.`last`, port 4002, stored in `repository` when given.
pub fn peer(last: u8) -> SharedPeer {
    SharedPeer::new(heartwood_p2p::Peer::new(
        IpAddr::from([192, 0, 2, last]),
        4002,
    ))
}

// ---- replies ----------------------------------------------------------------------------------

/// The configuration a status reports: version 4.3.1 on the devnet, with an API plugin on port
/// 4003.
pub fn peer_config(version: &str, nethash: &str) -> PeerConfig {
    PeerConfig {
        version: version.to_owned(),
        network: NetworkConfig {
            version: DEVNET.network.pub_key_hash,
            name: DEVNET.network.name.clone(),
            nethash: nethash.to_owned(),
            explorer: DEVNET.network.explorer.clone(),
            token: TokenConfig {
                name: DEVNET.network.token.clone(),
                symbol: DEVNET.network.symbol.clone(),
            },
        },
        plugins: vec![(
            "@solar-network/api".to_owned(),
            PluginConfig {
                enabled: true,
                port: 4003,
                estimate_total_count: Some(true),
            },
        )],
    }
}

/// A getStatus reply for `block` at `slot`, attested by `signers`.
pub fn status_reply(block: &Block, slot: i64, config: &PeerConfig, signers: &[KeyPair]) -> Vec<u8> {
    let state = SignedState {
        height: block.height(),
        forging_allowed: true,
        current_slot: slot,
        header: StatusHeaderData::from_block(block),
    };
    let preimage = status::signing_preimage(&state, config);
    let attestations: Vec<_> = signers
        .iter()
        .map(|keys| status::attest(&preimage, keys).unwrap())
        .collect();
    status::response(&state, config, &attestations).encode()
}

/// A getStatus reply of the devnet for `block` at `slot`, attested by `signers`.
pub fn devnet_status(block: &Block, slot: i64, signers: &[KeyPair]) -> Vec<u8> {
    status_reply(
        block,
        slot,
        &peer_config("4.3.1", &DEVNET.network.nethash),
        signers,
    )
}

/// Set `clock` to the first second of our slot `number` (8-second slots from the epoch).
pub fn at_slot(clock: &ManualClock, number: i64) {
    clock.set(EPOCH_MS + number * 8_000);
}
