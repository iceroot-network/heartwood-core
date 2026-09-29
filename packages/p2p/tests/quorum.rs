//! The network state and the forging quorum: `analyze` with the delegates on this node (the
//! attested keys intersected with the round's active set, duplicates kept, taken from the other
//! peers), the cold start, the minimum delegate reach and each count; and the fork health check's
//! local peer with and without attested keys, over a network monitor with a scripted client.
//! The network state is also compared with the reference's in class V30 (`quorum.analyze`), read
//! from `tests/vectors` of the crate, which must hold it (`make vectors-install`), or from the
//! directory in `P2P_VECTORS`, which may lack it (the test then says so and passes).

#![cfg(feature = "node")]

mod peer_fakes;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, BlockId, PublicKeyBytes, TransactionId};
use heartwood_kernel::ManualClock;
use heartwood_p2p::diversity::Sampler;
use heartwood_p2p::errors::{DownloadError, PingError};
use heartwood_p2p::monitor::{MonitorParts, NetworkStatus};
use heartwood_p2p::network_state::{AnalyzeInput, analyze};
use heartwood_p2p::peer::{FastVerification, Peer, PeerState, SharedPeer, Verification};
use heartwood_p2p::processor::AcceptOptions;
use heartwood_p2p::seams::{PeerAcceptor, PeerClient};
use heartwood_p2p::view::ChainView;
use heartwood_p2p::{
    AttestedKeys, BlocksRequest, DownloadedBlock, NetworkMonitor, NoAttestedKeys, P2pOptions,
    PeerRepository,
};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::internal::{self, NetworkStateStatus};
use heartwood_p2p_wire::messages::peer::StatusHeader;
use heartwood_p2p_wire::replies::{CommonBlock, PeerAddress};
use peer_fakes::{Chain, DEVNET, Events, Rules, START_MS, delegate};
use serde_json::{Value, json};

fn key(i: usize) -> PublicKeyBytes {
    delegate(i).public_key().to_compressed()
}

/// A chain at `height` with 60 registered delegates: delegate `i` has rank `i + 1`.
fn chain(height: u32) -> Chain {
    let mut chain = Chain::at(height);
    chain.delegates = (0..60).map(key).collect();
    chain
}

fn name(i: usize) -> String {
    format!("delegate_{i}")
}

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 0, 2, last])
}

/// A peer at `ip(last)` attesting `keys`, with a state at `height` in slot 125 (forging allowed),
/// not forked.
fn peer(last: u8, keys: &[usize], height: u32) -> Peer {
    let mut peer = Peer::new(ip(last), 4002);
    peer.public_keys = keys.iter().map(|&i| key(i)).collect();
    peer.state = Some(state(height, 125, true));
    peer.fast_verification = Some(FastVerification {
        my_height: height,
        their_height: height,
        highest_common_height: Some(height),
    });
    peer
}

fn state(height: u32, slot: i64, forging_allowed: bool) -> PeerState {
    PeerState {
        height,
        forging_allowed,
        current_slot: slot,
        header: StatusHeader {
            id: Some(format!("{height:064x}")),
            height: Some(height),
            generator_public_key: Some(key(3).to_hex()),
            ..StatusHeader::default()
        },
        total_amount: Amount::ZERO,
        total_fee: Amount::new(5),
        reward: Amount::ZERO,
    }
}

struct Case {
    chain: Chain,
    peers: Vec<Peer>,
    attested: Vec<PublicKeyBytes>,
    active: Vec<PublicKeyBytes>,
    cold_start: bool,
    reach: Option<u32>,
}

impl Case {
    fn new(peers: Vec<Peer>) -> Case {
        Case {
            chain: chain(10),
            peers,
            attested: Vec::new(),
            active: (0..53).map(key).collect(),
            cold_start: false,
            reach: Some(1),
        }
    }

    fn analyze(&self) -> heartwood_p2p::network_state::Analysis {
        analyze(&AnalyzeInput {
            chain: &self.chain,
            peers: &self.peers,
            attested: &self.attested,
            active_next_round: &self.active,
            port: 4002,
            now_ms: START_MS + 1_000,
            cold_start: self.cold_start,
            minimum_delegate_reach: self.reach,
        })
    }
}

#[test]
fn the_local_peer_carries_the_attested_keys_of_the_round_duplicates_kept() {
    // Delegate 7 ranks in the top 53 but is not in the round's set: it is no delegate on this
    // node. Delegate 1 is attested twice and counts twice; it is taken from the peer that holds it.
    let mut case = Case::new(vec![peer(1, &[1, 3], 10), peer(2, &[4], 10)]);
    case.active.retain(|k| *k != key(7));
    case.attested = [1, 2, 1, 7].map(key).to_vec();
    let analysis = case.analyze();
    assert_eq!(analysis.on_this_node, [1, 2, 1].map(key));
    let state = analysis.state;
    assert_eq!(state.status, NetworkStateStatus::Default);
    let details = &state.quorum_details;
    assert_eq!((details.has_quorum, details.no_quorum), (5, 0));
    let names = [
        "delegate_3",
        "delegate_4",
        "delegate_1",
        "delegate_2",
        "delegate_1",
    ];
    assert_eq!(details.delegates.has_quorum, names);
    // Every other delegate of the round did not respond; delegate 7 is not in the round.
    let silent = &details.delegates.no_response;
    assert_eq!(silent.len(), 52 - 4);
    assert!(!silent.contains(&name(7)) && !silent.contains(&name(1)));
    assert_eq!(state.node_height, Some(10));
    assert_eq!(state.last_generator.as_deref(), Some("delegate_0"));
    assert_eq!(state.last_slot_number, Some(10));
    assert!(state.can_forge(53));
    // Without the round's set, no delegate is on this node, and the peer keeps its key.
    case.active.clear();
    let analysis = case.analyze();
    assert!(analysis.on_this_node.is_empty());
    assert_eq!(analysis.state.quorum_details.has_quorum, 3);
}

#[test]
fn a_key_attested_27_times_passes_the_reach_and_the_quorum_on_its_own() {
    // A key counts as often as the forger's session proves it, as a key file of the reference
    // that lists it n times counts it n times: its holder can inflate its own node's counts, at
    // most 256 times (the declaration limit of a session), and nobody else's.
    let mut case = Case::new(Vec::new());
    case.reach = None;
    case.attested = vec![key(1); 27];
    let state = case.analyze().state;
    assert_eq!(state.status, NetworkStateStatus::Default);
    assert_eq!(state.quorum_details.has_quorum, 27);
    assert!(state.can_forge(53));
    case.attested.pop();
    let state = case.analyze().state;
    assert_eq!(state.status, NetworkStateStatus::BelowMinimumDelegates);
    assert!(!state.can_forge(53));
}

#[test]
fn a_cold_start_and_too_few_delegates_answer_their_status_with_no_count() {
    let mut case = Case::new(vec![peer(1, &[1, 2, 3], 10)]);
    case.cold_start = true;
    let state = case.analyze().state;
    assert_eq!(state.status, NetworkStateStatus::ColdStart);
    assert_eq!(state.quorum_details, Default::default());
    assert_eq!(
        state.last_block_id,
        Some(case.chain.last_block().id().to_hex())
    );
    // The default reach is half the active delegates plus one: 27.
    case.cold_start = false;
    case.reach = None;
    assert_eq!(
        case.analyze().state.status,
        NetworkStateStatus::BelowMinimumDelegates
    );
    case.peers = vec![
        peer(1, &(0..20).collect::<Vec<_>>(), 10),
        peer(2, &(20..27).collect::<Vec<_>>(), 10),
    ];
    assert_eq!(case.analyze().state.status, NetworkStateStatus::Default);
    // Keys ranked below 53 and unknown keys are no active delegate.
    case.peers[1].public_keys[6] = key(55);
    assert_eq!(
        case.analyze().state.status,
        NetworkStateStatus::BelowMinimumDelegates
    );
    case.reach = Some(26);
    assert_eq!(case.analyze().state.status, NetworkStateStatus::Default);
}

#[test]
fn peers_at_another_height_or_forked_count_against_the_quorum() {
    let mut over = peer(1, &[1, 2], 12);
    over.state.as_mut().unwrap().current_slot = 124;
    let under = peer(2, &[3], 9);
    let mut forked = peer(3, &[4], 10);
    forked.fast_verification = Some(FastVerification {
        my_height: 10,
        their_height: 10,
        highest_common_height: None,
    });
    let mut refusing = peer(4, &[5], 10);
    refusing.state.as_mut().unwrap().forging_allowed = false;
    let mut silent = peer(5, &[6], 10);
    silent.state = None;
    let inactive = peer(6, &[58], 12);
    let case = Case::new(vec![over, under, forked, refusing, silent, inactive]);
    let details = case.analyze().state.quorum_details;
    assert_eq!((details.has_quorum, details.no_quorum), (1, 4));
    assert_eq!((details.over_height, details.forked), (2, 1));
    // A different slot and forging not allowed are statistics; a peer without a state counts in
    // both, and in neither list.
    assert_eq!(
        (details.different_slot, details.forging_not_allowed),
        (3, 2)
    );
    assert_eq!(details.delegates.has_quorum, ["delegate_5"]);
    assert_eq!(
        details.delegates.no_quorum,
        ["delegate_1", "delegate_2", "delegate_3", "delegate_4"]
    );
    // The over-height header is kept by the peer's address, its amounts as text.
    let header = &details.over_height_block_headers["192.0.2.1"];
    assert_eq!(
        *header,
        json!({"id": format!("{:064x}", 12), "height": 12, "generatorPublicKey": key(3).to_hex(),
               "totalAmount": "0", "totalFee": "5", "reward": "0"})
    );
    assert_eq!(details.over_height_block_headers.len(), 1);
    // The answer's bytes carry the counts where the reference writes them.
    let json: Value = serde_json::from_slice(&internal::encode(&case.analyze().state)).unwrap();
    assert_eq!(json["quorumDetails"]["overHeight"], 2);
}

// ---- the fork health check ---------------------------------------------------------------------

/// A peer's answer to a ping: its height, its attested keys and its verification.
type Ping = (u32, Vec<PublicKeyBytes>, Option<Verification>);

/// A client whose pings set each peer's height, keys and verification from a script.
struct Client {
    pings: Mutex<HashMap<IpAddr, Ping>>,
}

impl PeerClient for Client {
    async fn ping(
        &self,
        peer: &SharedPeer,
        _timeout: Duration,
        _attest: bool,
        _force: bool,
        _skip_common_blocks: bool,
    ) -> Result<Option<PeerState>, PingError> {
        let script = self.pings.lock().unwrap().get(&peer.ip()).cloned();
        let (height, keys, verification) = script.ok_or(PingError::StatusResponse(peer.ip()))?;
        peer.with(|p| {
            p.state = Some(state(height, 125, true));
            p.public_keys = keys;
            p.verification = verification;
            p.stale = false;
        });
        Ok(Some(state(height, 125, true)))
    }
    async fn ping_ports(&self, _peer: &SharedPeer) {}
    async fn get_peers(&self, _peer: &SharedPeer, _silent: bool) -> Option<Vec<PeerAddress>> {
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
        _: &SharedPeer,
        _: BlocksRequest,
    ) -> Result<Vec<DownloadedBlock>, DownloadError> {
        Ok(Vec::new())
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
    fn would_throttle(&self, _: IpAddr, _: RouteId) -> bool {
        false
    }
    fn disconnect(&self, _: IpAddr) {}
    fn remove_peer(&self, _: IpAddr) {}
}

struct Acceptor;

impl PeerAcceptor for Acceptor {
    fn validate_peer_ip(&self, _: &mut PeerAddress, _: AcceptOptions) -> bool {
        false
    }
    async fn validate_and_accept(&self, _: PeerAddress, _: AcceptOptions) {}
}

struct Attested(Vec<PublicKeyBytes>);

impl AttestedKeys for Attested {
    fn attested(&self) -> Vec<PublicKeyBytes> {
        self.0.clone()
    }
}

fn forked_at(common: u32) -> Option<Verification> {
    Some(Verification {
        my_height: 120,
        their_height: 130,
        highest_common_height: common,
    })
}

/// A monitor at height 120 with two peers: 20 delegates on one forked at 100, 5 on one that is
/// not forked; with `attested` as the delegates on this node.
fn monitor(
    attested: Arc<dyn AttestedKeys>,
) -> (NetworkMonitor<Client, Acceptor, Rules>, Arc<PeerRepository>) {
    let repository = Arc::new(PeerRepository::new());
    let keys = |range: std::ops::Range<usize>| range.map(key).collect::<Vec<_>>();
    let pings = HashMap::from([
        (ip(1), (130, keys(0..20), forked_at(100))),
        (ip(2), (120, keys(20..25), forked_at(120))),
    ]);
    for last in [1, 2] {
        repository.set(SharedPeer::new(Peer::new(ip(last), 4002)));
    }
    let monitor = NetworkMonitor::new(MonitorParts {
        client: Arc::new(Client {
            pings: Mutex::new(pings),
        }),
        acceptor: Arc::new(Acceptor),
        rules: Arc::new(Rules::default()),
        repository: Arc::clone(&repository),
        chain: Arc::new(chain(120)) as Arc<dyn ChainView>,
        events: Arc::new(Events::default()),
        options: Arc::new(P2pOptions::default()),
        clock: Arc::new(ManualClock::new(START_MS)),
        sampler: Sampler::from_os().unwrap(),
        runtime: tokio::runtime::Handle::current(),
        attested,
    });
    (monitor, repository)
}

#[tokio::test(start_paused = true)]
async fn the_fork_health_check_counts_the_delegates_on_this_node_at_our_height() {
    // Without a delegate on this node: 25 delegates, too few to judge, so no fork.
    let (plain, _) = monitor(Arc::new(NoAttestedKeys));
    assert_eq!(
        plain.check_network_health(true).await,
        NetworkStatus::default()
    );
    // With six on this node, delegate 0 among them: the local peer at our height carries them,
    // and delegate 0 leaves the forked peer. 19 forked at 100 against 11 above it: a fork of 20
    // blocks.
    let on_node: Vec<PublicKeyBytes> = [0, 25, 26, 27, 28, 29].map(key).to_vec();
    let (with_keys, repository) = monitor(Arc::new(Attested(on_node)));
    let status = with_keys.check_network_health(true).await;
    assert_eq!(
        status,
        NetworkStatus {
            forked: true,
            blocks_to_rollback: Some(20)
        }
    );
    // The key stays removed from the peer until its next attestation.
    let forked = repository.get(ip(1)).unwrap().snapshot();
    assert_eq!(forked.public_keys, (1..20).map(key).collect::<Vec<_>>());
    // Keys that are no active delegate do not count on the local peer either.
    let (inactive, _) = monitor(Arc::new(Attested([55, 56].map(key).to_vec())));
    assert_eq!(
        inactive.check_network_health(true).await,
        NetworkStatus::default()
    );
}

// ---- class V30: the network state ---------------------------------------------------------------

const CLASS: &str = "V30-relay-forger";

/// The class's `quorum.analyze` records, or `None` when the directory `P2P_VECTORS` names lacks
/// the class. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("P2P_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(
        records
            .filter(|record| record["op"] == "quorum.analyze")
            .collect(),
    )
}

#[test]
fn the_network_state_matches_the_reference() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not in the directory P2P_VECTORS names");
        return;
    };
    assert!(!records.is_empty(), "{CLASS} has no quorum.analyze record");
    for record in &records {
        let height = u32::try_from(record["height"].as_u64().unwrap()).unwrap();
        let actual = answer(&record["input"], height);
        assert_eq!(actual, record["output"], "{}", record["name"]);
    }
}

/// The registered delegates of a record, by key: the name and the rank.
struct Registered {
    tip: Arc<Block>,
    delegates: HashMap<PublicKeyBytes, (String, Option<u32>)>,
}

impl ChainView for Registered {
    fn last_block(&self) -> Arc<Block> {
        Arc::clone(&self.tip)
    }
    fn last_blocks(&self) -> Vec<Arc<Block>> {
        vec![self.last_block()]
    }
    fn milestones(&self) -> &heartwood_crypto::Milestones {
        &DEVNET.milestones
    }
    fn network(&self) -> &heartwood_crypto::Network {
        &DEVNET.network
    }
    fn delegate_by_key(&self, key: &PublicKeyBytes) -> Option<heartwood_p2p::view::DelegateInfo> {
        let (username, rank) = self.delegates.get(key)?.clone();
        Some(heartwood_p2p::view::DelegateInfo { username, rank })
    }
    fn delegate_keys(&self) -> Vec<PublicKeyBytes> {
        self.delegates.keys().copied().collect()
    }
    fn is_synced(&self) -> bool {
        true
    }
    fn is_idle(&self) -> bool {
        true
    }
    fn block_ping(&self) -> Option<heartwood_p2p::view::BlockPing> {
        None
    }
}

fn public_key(value: &Value) -> PublicKeyBytes {
    PublicKeyBytes::from_hex(value.as_str().unwrap()).unwrap()
}

fn keys_of(value: &Value) -> Vec<PublicKeyBytes> {
    value.as_array().unwrap().iter().map(public_key).collect()
}

/// A peer of a record: its address, its keys, its state and its fast verification.
fn peer_of(value: &Value) -> Peer {
    let mut peer = Peer::new(value["ip"].as_str().unwrap().parse().unwrap(), 4002);
    peer.public_keys = keys_of(&value["publicKeys"]);
    if let Some(state) = value["state"].as_object() {
        let header = &state["header"];
        let text = |key: &str| header.get(key).and_then(Value::as_str).map(str::to_owned);
        let number = |key: &str| header.get(key).and_then(Value::as_u64).map(|n| n as u32);
        let amount = |key: &str| {
            Amount::parse(header.get(key).and_then(Value::as_str).unwrap_or("")).unwrap()
        };
        peer.state = Some(PeerState {
            height: state["height"].as_u64().unwrap() as u32,
            forging_allowed: state["forgingAllowed"].as_bool().unwrap(),
            current_slot: state["currentSlot"].as_i64().unwrap(),
            header: StatusHeader {
                id: text("id"),
                version: number("version"),
                timestamp: number("timestamp"),
                previous_block: text("previousBlock"),
                height: number("height"),
                number_of_transactions: number("numberOfTransactions"),
                total_amount: text("totalAmount"),
                total_fee: text("totalFee"),
                reward: text("reward"),
                payload_length: number("payloadLength"),
                payload_hash: text("payloadHash"),
                generator_public_key: text("generatorPublicKey"),
                block_signature: text("blockSignature"),
                ..StatusHeader::default()
            },
            total_amount: amount("totalAmount"),
            total_fee: amount("totalFee"),
            reward: amount("reward"),
        });
    }
    if let Some(fast) = value["fastVerification"].as_object() {
        let height = |key: &str| fast[key].as_u64().unwrap() as u32;
        peer.fast_verification = Some(FastVerification {
            my_height: height("myHeight"),
            their_height: height("theirHeight"),
            highest_common_height: (fast.get("highestCommonHeight"))
                .and_then(Value::as_u64)
                .map(|n| n as u32),
        });
    }
    peer
}

/// This crate's answer to a `quorum.analyze` input at `height` (see
/// `tools/oracle/lib/ops/relay.js` for the input): the delegates on this node are the declared
/// keys the forger lists, as a forger's session attests them.
fn answer(input: &Value, height: u32) -> Value {
    let bytes = hex::decode(input["tip"]["hex"].as_str().unwrap()).unwrap();
    let tip = Arc::new(Block::decode(&bytes, &DEVNET.milestones).unwrap());
    let delegates = (input["delegates"].as_array().unwrap().iter()).map(|delegate| {
        let rank = delegate["rank"].as_u64().map(|rank| rank as u32);
        let name = delegate["username"].as_str().unwrap().to_owned();
        (public_key(&delegate["publicKey"]), (name, rank))
    });
    let chain = Registered {
        tip,
        delegates: delegates.collect(),
    };
    let listed = keys_of(&input["forgerKeys"]);
    let attested: Vec<PublicKeyBytes> = (input["declared"].as_array().unwrap().iter())
        .map(|secret| {
            let secret = heartwood_crypto::SecretKey::from_hex(secret.as_str().unwrap()).unwrap();
            heartwood_crypto::KeyPair::from_secret_key(secret)
                .public_key()
                .to_compressed()
        })
        .filter(|key| listed.contains(key) && chain.delegate_by_key(key).is_some())
        .collect();
    let peers: Vec<Peer> = input["peers"]
        .as_array()
        .unwrap()
        .iter()
        .map(peer_of)
        .collect();
    let analysis = analyze(&AnalyzeInput {
        chain: &chain,
        peers: &peers,
        attested: &attested,
        active_next_round: &keys_of(&input["active"]),
        port: 4002,
        now_ms: input["now"].as_i64().unwrap(),
        cold_start: input["coldStart"].as_bool().unwrap(),
        minimum_delegate_reach: input
            .get("minimumDelegateReach")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
    });
    let active = u32::try_from(DEVNET.milestones.at(height).active_delegates()).unwrap();
    let peer_keys: Vec<Vec<String>> = (peers.iter())
        .map(|peer| {
            let kept = peer
                .public_keys
                .iter()
                .filter(|key| !analysis.on_this_node.contains(key));
            kept.map(PublicKeyBytes::to_hex).collect()
        })
        .collect();
    json!({
        "json": String::from_utf8(internal::encode(&analysis.state)).unwrap(),
        "canForge": analysis.state.can_forge(active),
        "peerKeys": peer_keys,
    })
}
