//! The network monitor's decisions and the download path, answered with `heartwood_p2p`:
//!
//! - `monitor.networkHeight` `{heights}` -> `{height}`: [`median_height`];
//! - `monitor.forkDecision` `{height, peers, delegates}` -> `{forked, blocksToRollback}`:
//!   [`fork_decision`] under the milestone at the case's height;
//! - `monitor.downloadPlan` `{peers, networkHeight, calls}` -> `{calls}`: the monitor's chunked
//!   download over a scripted client, once per call on the same monitor, with the orders the
//!   reference drew injected into its sampler;
//! - `block.fromListed` `{hex}` -> `{valid, downloaded, blocks}`: the codec, the reply check, the
//!   transactions read as `fromBytesUnsafe` reads them ([`read_listed`]), and [`to_block`].

use std::cmp::Ordering;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::{
    Aux, Block, BlockId, KeyPair, Milestones, Network, Params, PublicKeyBytes, Transaction,
    TransactionId,
};
use heartwood_kernel::{Clock, ManualClock};
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::download::read_listed;
use heartwood_p2p::errors::{DownloadError, PingError};
use heartwood_p2p::monitor::{ForkPeer, MonitorParts, active_count, fork_decision, median_height};
use heartwood_p2p::peer::{Peer, PeerState, SharedPeer, Verification};
use heartwood_p2p::processor::AcceptOptions;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient, PeerRules};
use heartwood_p2p::view::{BlockPing, ChainView, DelegateInfo, NoEvents};
use heartwood_p2p::{
    BlocksRequest, DownloadedBlock, NetworkMonitor, P2pOptions, PeerRepository, to_block,
};
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::{ListedBlock, get_blocks_decode_response};
use heartwood_p2p_wire::errors::ReplyRefused;
use heartwood_p2p_wire::messages::blocks::BlockHeader as ListedHeader;
use heartwood_p2p_wire::messages::blocks::PostBlockResponse;
use heartwood_p2p_wire::messages::peer::{GetCommonBlocksResponse, Peer as ListedPeer};
use heartwood_p2p_wire::replies::{self, CommonBlock, PeerAddress, UnconfirmedReply};
use heartwood_p2p_wire::requests::{ClientRequest, Headers};
use heartwood_p2p_wire::{DecodeError, RouteId};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{
    Env, Outcome, Run, documented, failed, has_form, hex_bytes, integer, params, rejected,
};
use crate::subject::{Case, Documented, Verdict};

/// The operations of the monitor.
pub(super) const OPS: &[(&str, Run)] = &[
    ("monitor.networkHeight", network_height),
    ("monitor.forkDecision", fork_decision_case),
    ("monitor.downloadPlan", download_plan),
    ("block.fromListed", from_listed),
];

fn array<'a>(value: &'a Value, name: &str) -> Outcome<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| failed(format!("{name} is not an array")))
}

fn height_of(value: &Value, name: &str) -> Outcome<Option<u32>> {
    match value {
        Value::Null => Ok(None),
        other => other
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| failed(format!("{name} is not a height"))),
    }
}

fn network_height(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let heights = array(&case.input["heights"], "input.heights")?
        .iter()
        .map(|h| height_of(h, "a height"))
        .collect::<Outcome<Vec<_>>>()?;
    Ok(Verdict::Output(json!({ "height": median_height(heights) })))
}

fn fork_decision_case(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let active = params(&milestones, case)?.active_delegates();
    let height: u32 = integer(case.input, "height")?;
    let key = |text: &str| {
        PublicKeyBytes::from_hex(text).map_err(|e| failed(format!("a public key {text}: {e}")))
    };
    let mut ranks: HashMap<PublicKeyBytes, Option<u32>> = HashMap::new();
    let delegates = case.input["delegates"]
        .as_object()
        .ok_or_else(|| failed("input.delegates is not an object"))?;
    for (text, rank) in delegates {
        ranks.insert(key(text)?, height_of(rank, "a rank")?);
    }
    let rank_of = |k: &PublicKeyBytes| ranks.get(k).copied().flatten();
    let mut peers = Vec::new();
    for peer in array(&case.input["peers"], "input.peers")? {
        let keys = array(&peer["publicKeys"], "publicKeys")?
            .iter()
            .map(|k| key(k.as_str().unwrap_or_default()))
            .collect::<Outcome<Vec<_>>>()?;
        let v = &peer["verification"];
        let verification = if v.is_null() {
            None
        } else {
            Some(Verification {
                my_height: integer(v, "myHeight")?,
                their_height: integer(v, "theirHeight")?,
                highest_common_height: integer(v, "highestCommonHeight")?,
            })
        };
        peers.push(ForkPeer {
            active_delegates: active_count(&keys, rank_of, active),
            height: height_of(&peer["height"], "a peer's height")?,
            verification,
        });
    }
    let status = fork_decision(&peers, height, active);
    Ok(Verdict::Output(json!({
        "forked": status.forked,
        "blocksToRollback": status.blocks_to_rollback,
    })))
}

// ---- the download plan ----------------------------------------------------------------------

/// What one peer serves in a call: blocks up to `top`, a reader failure at the chunk starts of
/// `fail`.
#[derive(Debug, Clone, Default)]
struct Answer {
    top: u32,
    fail: Vec<u32>,
}

/// The client of a plan: the call's answers and throttled peers, and the requests it got.
#[derive(Default)]
struct PlanClient {
    answers: Mutex<HashMap<IpAddr, Answer>>,
    throttled: Mutex<Vec<IpAddr>>,
    requests: Mutex<Vec<(u32, IpAddr)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PeerClient for PlanClient {
    async fn ping(
        &self,
        _: &SharedPeer,
        _: Duration,
        _: bool,
        _: bool,
        _: bool,
    ) -> Result<Option<PeerState>, PingError> {
        Ok(None)
    }
    async fn ping_ports(&self, _: &SharedPeer) {}
    async fn get_peers(&self, _: &SharedPeer, _: bool) -> Option<Vec<PeerAddress>> {
        None
    }
    async fn has_common_blocks(
        &self,
        _: &SharedPeer,
        _: &[BlockId],
        _: Duration,
    ) -> Option<CommonBlock> {
        None
    }
    async fn get_peer_blocks(
        &self,
        peer: &SharedPeer,
        request: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        let from = request.from_height;
        lock(&self.requests).push((from, peer.ip()));
        let answer = lock(&self.answers)
            .get(&peer.ip())
            .cloned()
            .unwrap_or_default();
        if answer.fail.contains(&from) {
            return Err(reader_failure(from));
        }
        let last = answer.top.min(from.saturating_add(request.block_limit));
        let block = |height: u32| DownloadedBlock {
            header: ListedBlock {
                header: ListedHeader {
                    height: Some(height),
                    ..ListedHeader::default()
                },
                ..ListedBlock::default()
            },
            transactions: Some(Vec::new()),
            ip: peer.ip(),
        };
        Ok((from.saturating_add(1)..=last).map(block).collect())
    }
    async fn get_unconfirmed_transactions(
        &self,
        _: &SharedPeer,
        _: &[TransactionId],
    ) -> Vec<Vec<u8>> {
        Vec::new()
    }
    async fn post_block(&self, _: &SharedPeer, _: &Block) {}
    fn post_transactions(&self, _: &SharedPeer, _: Arc<Vec<Vec<u8>>>) {}
    fn would_throttle(&self, ip: IpAddr, route: RouteId) -> bool {
        route == RouteId::GetBlocks && lock(&self.throttled).contains(&ip)
    }
    fn disconnect(&self, _: IpAddr) {}
    fn remove_peer(&self, _: IpAddr) {}
}

/// The download error of a transaction the reader refuses, for a chunk from `from`.
fn reader_failure(from: u32) -> DownloadError {
    DownloadError::Transaction {
        height: from.saturating_add(1),
        source: heartwood_crypto::errors::DecodeError::SignatureBytes,
    }
}

/// The peer rules of a plan: the download calls none.
struct NoRules;

impl PeerRules for NoRules {
    fn encode_request(&self, _: &ClientRequest, _: &Headers) -> Vec<u8> {
        Vec::new()
    }
    fn check_peers(&self, _: &[ListedPeer]) -> Result<Vec<PeerAddress>, ReplyRefused> {
        Ok(Vec::new())
    }
    fn check_common_blocks(
        &self,
        _: &GetCommonBlocksResponse,
    ) -> Result<Option<CommonBlock>, ReplyRefused> {
        Ok(None)
    }
    fn check_blocks(&self, _: &mut [ListedBlock]) -> Result<(), ReplyRefused> {
        Ok(())
    }
    fn check_post_block(&self, _: &PostBlockResponse) -> Result<Option<u32>, ReplyRefused> {
        Ok(None)
    }
    fn check_unconfirmed(&self, _: &UnconfirmedReply, _: usize) -> Result<(), ReplyRefused> {
        Ok(())
    }
    fn check_post_transactions(&self, _: &[String]) -> Result<(), ReplyRefused> {
        Ok(())
    }
    fn throttle(&self, _: IpAddr, _: RouteId, _: Instant) -> Option<Duration> {
        None
    }
    fn would_throttle(&self, _: IpAddr, _: RouteId, _: Instant) -> bool {
        false
    }
    fn is_valid_peer(&self, _: &mut String, _: bool, _: &[IpAddr]) -> bool {
        false
    }
    fn is_valid_version(&self, _: Option<&str>, _: &Params, _: &[String], _: &str) -> bool {
        false
    }
    fn same_subnet(&self, _: IpAddr, _: IpAddr) -> bool {
        false
    }
    fn compare_versions(&self, _: &str, _: &str) -> Option<Ordering> {
        None
    }
}

/// The acceptance of a plan: never asked.
struct NoAcceptor;

impl PeerAcceptor for NoAcceptor {
    fn validate_peer_ip(&self, _: &mut PeerAddress, _: AcceptOptions) -> bool {
        false
    }
    async fn validate_and_accept(&self, _: PeerAddress, _: AcceptOptions) {}
}

/// The chain of a plan: the download reads nothing of it.
struct PlanChain {
    milestones: Milestones,
    network: Network,
    last: Arc<Block>,
}

impl ChainView for PlanChain {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.last)
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![Arc::clone(&self.last)]
    }
    fn milestones(&self) -> &Milestones {
        &self.milestones
    }
    fn network(&self) -> &Network {
        &self.network
    }
    fn delegate_by_key(&self, _: &PublicKeyBytes) -> Option<DelegateInfo> {
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

/// The [from, to] of a chunk key (`[1,201, ..]`), to null for `..`.
fn range_of(key: &str) -> Value {
    let number = |text: &str| -> Value {
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        digits.parse::<u64>().map_or(Value::Null, |n| json!(n))
    };
    let inner = key.trim_start_matches('[').trim_end_matches(']');
    let (from, to) = inner.split_once(", ").unwrap_or((inner, ".."));
    json!([
        number(from),
        if to == ".." { Value::Null } else { number(to) }
    ])
}

fn download_plan(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let devnet = env.devnet()?;
    let input = case.input;
    let network_height: u32 = integer(input, "networkHeight")?;
    let calls = array(&input["calls"], "input.calls")?;
    let mut orders = Vec::new();
    for call in calls {
        for order in array(&call["orders"], "orders")? {
            let order = array(order, "an order")?
                .iter()
                .map(|i| i.as_u64().and_then(|i| usize::try_from(i).ok()))
                .collect::<Option<Vec<usize>>>()
                .ok_or_else(|| failed("an order holds a value that is not an index"))?;
            orders.push(order);
        }
    }
    let repository = Arc::new(PeerRepository::new());
    for peer in array(&input["peers"], "input.peers")? {
        let ip: IpAddr = peer["ip"]
            .as_str()
            .and_then(|ip| ip.parse().ok())
            .ok_or_else(|| failed("a peer's ip"))?;
        let port: u16 = integer(peer, "port")?;
        let mut stored = Peer::new(ip, port);
        stored.state = Some(state(network_height));
        repository.set(SharedPeer::new(stored));
    }
    let keys = KeyPair::from_passphrase("download plan").map_err(|e| failed(e.to_string()))?;
    let draft = BlockDraft {
        version: 0,
        timestamp: 0,
        height: 1,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: [0; 32],
    };
    let last = factory::make(draft, Vec::new(), &keys, Aux::random(), &devnet.milestones)
        .map_err(|e| failed(e.to_string()))?;
    let client = Arc::new(PlanClient::default());
    let sampler = Sampler::scripted(orders);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let monitor = Arc::new(NetworkMonitor::new(MonitorParts {
        client: Arc::clone(&client),
        acceptor: Arc::new(NoAcceptor),
        rules: Arc::new(NoRules),
        repository: Arc::clone(&repository),
        chain: Arc::new(PlanChain {
            milestones: devnet.milestones.clone(),
            network: devnet.network.clone(),
            last: Arc::new(last),
        }),
        events: Arc::new(NoEvents),
        options: Arc::new(P2pOptions::default()),
        clock: Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
        sampler: sampler.clone(),
        runtime: runtime.handle().clone(),
    }));
    let mut outputs = Vec::new();
    for call in calls {
        if let Some(height) = height_of(&call["networkHeight"], "networkHeight")? {
            for peer in repository.peers() {
                peer.with(|p| p.state = Some(state(height)));
            }
        }
        let mut answers = HashMap::new();
        if let Some(given) = call["answers"].as_object() {
            for (ip, answer) in given {
                let ip: IpAddr = ip.parse().map_err(|_| failed("an answer's ip"))?;
                let fail = answer["fail"].as_array().map_or(Ok(Vec::new()), |f| {
                    f.iter().map(|v| height_of(v, "a failing chunk")).collect()
                })?;
                let top = height_of(&answer["top"], "top")?.unwrap_or(0);
                answers.insert(
                    ip,
                    Answer {
                        top,
                        fail: fail.into_iter().flatten().collect(),
                    },
                );
            }
        }
        *lock(&client.answers) = answers;
        let throttled = call["throttled"].as_array().map_or_else(Vec::new, |t| {
            t.iter()
                .filter_map(|ip| ip.as_str()?.parse().ok())
                .collect()
        });
        *lock(&client.throttled) = throttled;
        lock(&client.requests).clear();
        let from: u32 = integer(call, "from")?;
        let parallel: usize = integer(call, "maxParallel")?;
        let throttle = call["checkThrottle"] == json!(true);
        let wait = Some(Duration::from_secs(30));
        let blocks = runtime
            .block_on(monitor.download_blocks_from_height(from, parallel, false, wait, throttle));
        let mut requests: Vec<(u32, Vec<String>)> = Vec::new();
        for (start, ip) in lock(&client.requests).iter() {
            match requests.iter_mut().find(|(s, _)| s == start) {
                Some((_, ips)) => ips.push(ip.to_string()),
                None => requests.push((*start, vec![ip.to_string()])),
            }
        }
        requests.sort_by_key(|(start, _)| *start);
        let mut runs: Vec<(u32, u32, IpAddr)> = Vec::new();
        for block in &blocks {
            match runs.last_mut() {
                Some(run) if run.2 == block.ip && run.1.checked_add(1) == Some(block.height()) => {
                    run.1 = block.height();
                }
                _ => runs.push((block.height(), block.height(), block.ip)),
            }
        }
        outputs.push(json!({
            "requests": requests.iter().map(|(s, ips)| json!([s, ips])).collect::<Vec<_>>(),
            "blocks": runs.iter().map(|(a, b, ip)| json!([a, b, ip.to_string()])).collect::<Vec<_>>(),
            "cached": monitor.cached_chunks().iter().map(|k| range_of(k)).collect::<Vec<_>>(),
            "nextChunkSize": monitor.chunk_size(),
        }));
    }
    let (left, misfits) = sampler.script_left();
    if left != 0 || misfits != 0 {
        return Err(failed(format!(
            "the shuffles did not take the orders given: {left} left, {misfits} misfits"
        )));
    }
    Ok(Verdict::Output(json!({ "calls": outputs })))
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

// ---- a listed block ---------------------------------------------------------------------------

fn from_listed(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let params = params(&milestones, case)?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let refused = || Verdict::Output(json!({ "valid": false }));
    let mut listed = match get_blocks_decode_response(&bytes) {
        Ok(listed) => listed,
        Err(DecodeError::AmountTooWide) => {
            return Ok(Verdict::DiffersWhereShown {
                difference: Documented::WideAmount,
                verdict: Box::new(refused()),
            });
        }
        Err(_) => return Ok(refused()),
    };
    // Heartwood skips protobuf groups at any depth, where the reference's stack overflows.
    let differs = |verdict: Verdict| Verdict::DiffersWhereShown {
        difference: Documented::DeepGroups,
        verdict: Box::new(verdict),
    };
    if replies::check_blocks(&mut listed).is_err() {
        return Ok(differs(refused()));
    }
    let from = IpAddr::from([192, 0, 2, 1]);
    let mut downloaded = Vec::with_capacity(listed.len());
    for block in listed {
        let raw = block.transactions.clone();
        match read_listed(block, from, false, params) {
            Ok(read) => downloaded.push(read),
            Err(DownloadError::Transaction { source, .. }) => {
                let verdict = match documented(&source) {
                    Some(difference) => Verdict::RefusedByDesign {
                        difference,
                        holds: raw.iter().any(|bytes| {
                            Transaction::decode_trusted(bytes, params).is_err()
                                && has_form(difference, bytes)
                        }),
                    },
                    None => Verdict::Output(json!({ "valid": true, "downloaded": false })),
                };
                return Ok(differs(verdict));
            }
            // The reader counts no transactions: only the communicator refuses too many.
            Err(error @ DownloadError::TooManyTransactions { .. }) => {
                return Err(failed(format!("the reader refused a block: {error}")));
            }
        }
    }
    let blocks: Vec<Value> = downloaded
        .iter()
        .map(|block| match to_block(block, &milestones) {
            Ok(block) => json!({ "id": block.id().to_hex(), "accepted": true }),
            Err(_) => json!({ "id": null, "accepted": false }),
        })
        .collect();
    Ok(differs(Verdict::Output(json!({
        "valid": true,
        "downloaded": true,
        "blocks": blocks,
    }))))
}
