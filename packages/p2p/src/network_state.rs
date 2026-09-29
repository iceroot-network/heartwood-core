//! The network state, as the reference's `NetworkState.analyze` builds it for getNetworkState:
//! the tip and the slot; the peers; the delegates on this node (the attested keys that are also
//! active in the round of the tip plus one, in declared order, duplicates kept) as a local peer
//! `127.0.0.1:<P2P port>` with the tip's state, those keys removed from every other peer; the
//! peers without an active delegate dropped. A cold start answers `ColdStart` and ends it; fewer
//! active delegates across the peers than `minimumDelegateReach` answer `BelowMinimumDelegates`;
//! otherwise each peer counts toward the quorum by its active delegates.
//!
//! A key is active when its delegate has a rank of at most the milestone's `activeDelegates` at
//! the tip, as the reference's `Peer.activeDelegates` decides.

use heartwood_crypto::PublicKeyBytes;
use heartwood_crypto::crypto::slots;
use heartwood_p2p_wire::Proto;
use heartwood_p2p_wire::internal::{NetworkState, NetworkStateStatus, QuorumDetails};
use serde_json::{Map, Value};

use crate::attestation::current_slot;
use crate::peer::{Peer, PeerState};
use crate::view::ChainView;

/// What the analysis reads.
pub struct AnalyzeInput<'a> {
    /// The node's chain.
    pub chain: &'a dyn ChainView,
    /// The peers, their attested keys as the repository holds them.
    pub peers: &'a [Peer],
    /// The attested keys of the node's forger.
    pub attested: &'a [PublicKeyBytes],
    /// The active delegates' keys of the round of the tip plus one.
    pub active_next_round: &'a [PublicKeyBytes],
    /// The node's P2P port.
    pub port: u16,
    /// The time, in milliseconds since the Unix epoch.
    pub now_ms: i64,
    /// Whether the node is in its cold start.
    pub cold_start: bool,
    /// The `minimumDelegateReach` option, if set.
    pub minimum_delegate_reach: Option<u32>,
}

/// What the analysis found, and the keys to remove from the repository's peers until their next
/// attestation, as the reference reassigns them.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The network state.
    pub state: NetworkState,
    /// The delegates on this node, removed from the other peers.
    pub on_this_node: Vec<PublicKeyBytes>,
}

/// A peer as the quorum counts it.
struct Counted {
    /// The address, which keys the over-height headers.
    ip: String,
    /// The active delegates' keys it carries.
    keys: Vec<PublicKeyBytes>,
    /// The height of its state's header and the header, when it has one.
    header: Option<(u32, Value)>,
    /// Whether its last fast verification found a fork.
    forked: bool,
    /// Its state's slot.
    current_slot: Option<i64>,
    /// Whether its state allowed forging.
    forging_allowed: bool,
}

impl Counted {
    fn of(peer: &Peer, on_this_node: &[PublicKeyBytes]) -> Counted {
        let state = peer.state.as_ref();
        let header = state.and_then(|state| Some((state.header.height?, header_json(state))));
        Counted {
            ip: peer.ip.to_string(),
            keys: (peer.public_keys.iter())
                .filter(|key| !on_this_node.contains(key))
                .copied()
                .collect(),
            header,
            forked: peer.fast_verification.is_some_and(|fast| fast.forked()),
            current_slot: state.map(|state| state.current_slot),
            forging_allowed: state.is_some_and(|state| state.forging_allowed),
        }
    }
}

/// The analysis of `input`.
pub fn analyze(input: &AnalyzeInput<'_>) -> Analysis {
    let chain = input.chain;
    let (tip, milestones) = (chain.last_block(), chain.milestones());
    let height = tip.height();
    let active = milestones.at(height).active_delegates();
    let slot = current_slot(milestones, input.now_ms);
    let on_this_node: Vec<PublicKeyBytes> = (input.attested.iter())
        .filter(|key| input.active_next_round.contains(key))
        .copied()
        .collect();
    let mut peers: Vec<Counted> = (input.peers.iter())
        .map(|peer| Counted::of(peer, &on_this_node))
        .collect();
    if !on_this_node.is_empty() {
        peers.push(Counted {
            ip: "127.0.0.1".into(),
            keys: on_this_node.clone(),
            header: Some((height, Value::Null)),
            forked: false,
            current_slot: slot.map(|slot| slot.slot_number),
            forging_allowed: slot.is_some_and(|slot| slot.forging_status),
        });
    }
    let rank = |key: &PublicKeyBytes| chain.delegate_by_key(key).and_then(|d| d.rank);
    for peer in &mut peers {
        peer.keys
            .retain(|key| rank(key).is_some_and(|rank| u64::from(rank) <= active));
    }
    peers.retain(|peer| !peer.keys.is_empty());
    let reach = input
        .minimum_delegate_reach
        .map_or((active / 2).saturating_add(1), u64::from);
    let reached = peers.iter().map(|peer| peer.keys.len() as u64).sum::<u64>();
    let status = match () {
        () if input.cold_start => NetworkStateStatus::ColdStart,
        () if reached < reach => NetworkStateStatus::BelowMinimumDelegates,
        () => NetworkStateStatus::Default,
    };
    let generator = chain.delegate_by_key(&tip.header().generator_public_key);
    let block_time = milestones.at(1).block_time();
    let mut state = NetworkState {
        status,
        node_height: Some(height),
        last_block_id: Some(tip.id().to_hex()),
        last_generator: generator.map(|delegate| delegate.username),
        last_slot_number: Some(slots::slot_number(
            i64::from(tip.header().timestamp),
            block_time,
        )),
        quorum_details: QuorumDetails::default(),
    };
    if status == NetworkStateStatus::Default {
        let name = |key: &PublicKeyBytes| chain.delegate_by_key(key).map(|d| d.username);
        let details = &mut state.quorum_details;
        for peer in &peers {
            count(
                details,
                peer,
                height,
                slot.map(|slot| slot.slot_number),
                &name,
            );
        }
        let lists = &details.delegates;
        let answered =
            |name: &String| lists.has_quorum.contains(name) || lists.no_quorum.contains(name);
        let silent = (input.active_next_round.iter()).filter_map(name);
        details.delegates.no_response = silent.filter(|name| !answered(name)).collect();
    }
    Analysis {
        state,
        on_this_node,
    }
}

/// The reference's `update` of the counts with `peer`, at our height and slot.
fn count(
    details: &mut QuorumDetails,
    peer: &Counted,
    height: u32,
    slot: Option<i64>,
    name: &dyn Fn(&PublicKeyBytes) -> Option<String>,
) {
    let increment = u32::try_from(peer.keys.len()).unwrap_or(u32::MAX);
    let add = |count: &mut u32| *count = count.saturating_add(increment);
    let names = peer.keys.iter().filter_map(name);
    match &peer.header {
        Some((at, header)) if *at != height => {
            add(&mut details.no_quorum);
            details.delegates.no_quorum.extend(names);
            if *at > height {
                add(&mut details.over_height);
                let headers = &mut details.over_height_block_headers;
                headers.insert(peer.ip.clone(), header.clone());
            }
        }
        Some(_) if peer.forked => {
            add(&mut details.no_quorum);
            add(&mut details.forked);
            details.delegates.no_quorum.extend(names);
        }
        Some(_) => {
            add(&mut details.has_quorum);
            details.delegates.has_quorum.extend(names);
        }
        None => {}
    }
    if peer.current_slot.is_none() || peer.current_slot != slot {
        add(&mut details.different_slot);
    }
    if !peer.forging_allowed {
        add(&mut details.forging_not_allowed);
    }
}

/// A peer's state header as the reference keeps it and writes it: the decoded header's own keys
/// in their order, with the three amounts as decimal text.
fn header_json(state: &PeerState) -> Value {
    let header = &state.header;
    let amounts = [
        ("totalAmount", state.total_amount),
        ("totalFee", state.total_fee),
        ("reward", state.reward),
    ];
    let mut json = Map::new();
    for key in header
        .own_keys()
        .into_iter()
        .chain(amounts.map(|(key, _)| key))
    {
        let text = |value: &Option<String>| value.clone().map(Value::from);
        let value = match key {
            "id" => text(&header.id),
            "version" => header.version.map(Value::from),
            "timestamp" => header.timestamp.map(Value::from),
            "previousBlock" => text(&header.previous_block),
            "height" => header.height.map(Value::from),
            "numberOfTransactions" => header.number_of_transactions.map(Value::from),
            "payloadLength" => header.payload_length.map(Value::from),
            "payloadHash" => text(&header.payload_hash),
            "generatorPublicKey" => text(&header.generator_public_key),
            "blockSignature" => text(&header.block_signature),
            _ => amounts
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, amount)| Value::from(amount.to_string())),
        };
        if let Some(value) = value.filter(|_| !json.contains_key(key)) {
            json.insert(key.to_owned(), value);
        }
    }
    Value::Object(json)
}
