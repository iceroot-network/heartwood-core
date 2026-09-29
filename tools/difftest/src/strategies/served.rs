//! P20, the served answers: random requests of the seven public routes within their schemas, over
//! a chain of signed blocks, the synthetic store of full blocks, random peer repositories and
//! pools (`p2p.controller`); and the property's reach: each public route and the getBlocks cut
//! reached often enough.
//!
//! The chain is the harness's own: blocks 2 to 16, signed by four delegates, the first ones
//! carrying transfers. A case names its tip and keeps the blocks up to it as the store's rows.
//! The cases stay away from the answers Heartwood gives differently on purpose: no IPv4-mapped
//! address, no peer without a latency, and no block whose header the schema refuses.

use std::collections::BTreeMap;
use std::sync::Arc;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::blocks::serialiser;
use heartwood_crypto::crypto::hash::{sha256, sha256_concat};
use heartwood_crypto::crypto::slots;
use heartwood_crypto::transactions::builders::TransactionBuilder;
use heartwood_crypto::transactions::types::{Memo, TransferItem};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, Network};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::codecs::blocks::{
    get_blocks_decode_request, get_blocks_encode_request, post_block_encode_request,
};
use heartwood_p2p_wire::codecs::peer::{
    get_common_blocks_encode_request, get_peers_encode_request, get_status_encode_request,
};
use heartwood_p2p_wire::codecs::transactions::{
    PostTransactionsInput, UnconfirmedRequestInput, get_unconfirmed_transactions_encode_request,
    post_transactions_encode_request,
};
use heartwood_p2p_wire::messages::blocks::{GetBlocksRequest, PostBlockRequest};
use heartwood_p2p_wire::messages::peer::{
    GetCommonBlocksRequest, GetPeersRequest, GetStatusRequest,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::keys;
use super::transactions::{AUX, TxContext, TxSpec};
use crate::report::{CaseResult, Outcome};
use crate::vectors::Expected;

/// The heights of the harness's chain.
const FIRST: u32 = 2;
const LAST: u32 = 16;
/// The highest tip a case draws; the block above it is the next block a case can post.
const TIP_TOP: u32 = LAST - 1;
/// The fee of the chain's transactions: above every floor of the devnet.
const FEE: u64 = 20_000_000;
/// The rewards a block carries: the ordinary one and the secondary one.
const REWARDS: [u64; 2] = [200_000_000, 180_000_000];
/// The number of delegates that sign the chain.
const DELEGATES: u32 = 4;
/// The address that reaches the routes as the forger.
const FORGER: &str = "127.0.0.1";
/// The rows the synthetic store holds, from this height.
const SYNTHETIC_FROM: u32 = 2;
const SYNTHETIC_ROWS: u32 = 12;
/// The reference's default block limit of getBlocks.
const DEFAULT_LIMIT: u32 = 400;

/// A block of the chain.
struct Made {
    id: String,
    hex: String,
    header_hex: String,
    generator: String,
    transactions: Vec<Vec<u8>>,
}

/// What the cases are made of.
struct World {
    /// The blocks 2 to 16 at index `height − 2`, and the same blocks at the secondary reward.
    chain: Vec<Made>,
    reduced: Vec<Made>,
    /// Every transaction of the chain.
    transactions: Vec<Vec<u8>>,
    /// The transaction of the synthetic store, hex.
    full_transaction: String,
    epoch_ms: i64,
    network: Network,
    delegates: Vec<(String, String)>,
}

fn delegate_keys(n: u32) -> heartwood_crypto::KeyPair {
    keys(&format!("served delegate {n}"))
}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(&sha256(bytes))
}

impl World {
    fn new(context: &TxContext) -> Option<World> {
        let milestones = &context.milestones;
        let sender = keys("served sender");
        let mut nonce = 0u64;
        let mut make = |height: u32, previous: [u8; 32], reward: u64, count: usize| {
            let params = milestones.at(height);
            let mut transactions = Vec::new();
            for _ in 0..count {
                nonce += 1;
                let recipient = context.recipients[(nonce as usize) % context.recipients.len()];
                let spec = TxSpec::Transfer(vec![(recipient, 1_000 + nonce)]);
                transactions.push(spec.build(&sender, None, nonce, FEE, params)?);
            }
            let draft = BlockDraft {
                version: 0,
                timestamp: height * 8,
                height,
                previous_block: previous,
                number_of_transactions: u32::try_from(count).ok()?,
                total_amount: transactions.iter().map(|tx| tx.amount()).sum(),
                total_fee: transactions.iter().map(|tx| tx.fee()).sum(),
                reward,
                payload_length: u32::try_from(count * 32).ok()?,
                payload_hash: sha256_concat(
                    transactions.iter().map(|tx| tx.id().as_bytes().as_slice()),
                ),
            };
            let keys = delegate_keys(height % DELEGATES);
            let block = factory::make(
                draft,
                transactions,
                &keys,
                Aux::fixed([0x21; 32]),
                milestones,
            )
            .ok()?;
            let bytes = serialiser::serialise(&block);
            Some((
                *block.id().as_bytes(),
                Made {
                    id: block.id().to_hex(),
                    hex: hex::encode(&bytes),
                    header_hex: hex::encode(&serialiser::serialise_header(block.header(), true)),
                    generator: keys.public_key().to_compressed().to_hex(),
                    transactions: block
                        .transactions()
                        .iter()
                        .map(|tx| tx.bytes().to_vec())
                        .collect(),
                },
            ))
        };
        let (mut chain, mut reduced) = (Vec::new(), Vec::new());
        let mut previous = [0x11; 32];
        for height in FIRST..=LAST {
            let count = usize::try_from(height % 3).ok()?;
            let (id, block) = make(height, previous, REWARDS[0], count)?;
            chain.push(block);
            reduced.push(make(height, previous, REWARDS[1], count)?.1);
            previous = id;
        }
        let transactions = chain
            .iter()
            .flat_map(|block| block.transactions.clone())
            .collect();
        let params = milestones.at(FIRST);
        let items: Vec<TransferItem> = (0..256usize)
            .map(|i| TransferItem {
                amount: 1_000 + i as u64,
                recipient: context.recipients[i],
            })
            .collect();
        let memo = Memo::new(&"m".repeat(255)).ok()?;
        let full = TransactionBuilder::transfer(items)
            .network(params.network())
            .nonce(1)
            .fee(100_000_000)
            .memo(memo)
            .build(&keys("full block sender"), Aux::fixed(AUX), params)
            .ok()?;
        let list: Value = serde_json::from_str(&context.milestones_text).ok()?;
        Some(World {
            delegates: (0..DELEGATES)
                .map(|n| {
                    (
                        delegate_keys(n).public_key().to_compressed().to_hex(),
                        format!("served_delegate_{n}"),
                    )
                })
                .collect(),
            chain,
            reduced,
            transactions,
            full_transaction: hex::encode(full.bytes()),
            epoch_ms: slots::epoch_unix_ms(list[0]["epoch"].as_str()?)?,
            network: context.network.clone(),
        })
    }

    fn block(&self, height: u32) -> &Made {
        &self.chain[(height - FIRST) as usize]
    }

    /// The store of a node at `tip` that keeps the rows from `low` below it, with `memory`
    /// heights in memory.
    fn store(&self, tip: u32, low: u32, memory: u32) -> Value {
        let mut store = json!({
            "tip": self.block(tip).hex,
            "blocks": (low..tip).map(|h| self.block(h).hex.clone()).collect::<Vec<_>>(),
        });
        if memory != 100 {
            store["memory"] = json!(memory);
        }
        store
    }

    /// The synthetic store: the last block of the chain as the tip and rows of full blocks.
    fn synthetic(&self, per_block: u32) -> Value {
        json!({
            "tip": self.block(LAST).hex,
            "synthetic": {
                "from": SYNTHETIC_FROM, "count": SYNTHETIC_ROWS, "perBlock": per_block,
                "transaction": self.full_transaction,
            },
        })
    }
}

/// The input of a route: the request's bytes and the caller, with `fields` of the input.
fn input(route: RouteId, request: &[u8], ip: &str, fields: Value) -> Value {
    let mut input = json!({
        "route": route.id(),
        "request": hex::encode(request),
        "ip": ip,
        "remoteAccess": [FORGER],
    });
    if let (Some(input), Value::Object(fields)) = (input.as_object_mut(), fields) {
        input.extend(fields);
    }
    input
}

fn caller() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just("10.0.0.9".to_owned()),
        1 => Just("192.168.4.7".to_owned()),
        1 => Just("2001:db8::7".to_owned()),
        2 => Just(FORGER.to_owned()),
    ]
}

fn tip() -> impl Strategy<Value = u32> {
    prop_oneof![1 => Just(FIRST), 1 => Just(TIP_TOP), 6 => FIRST..=TIP_TOP]
}

/// The lowest row a store keeps below `tip` and the heights in memory.
fn store_shape(tip: u32) -> impl Strategy<Value = (u32, u32)> {
    (
        prop_oneof![5 => Just(FIRST), 1 => FIRST..=tip],
        prop_oneof![7 => Just(100u32), 2 => 1u32..=6, 1 => Just(1u32)],
    )
}

fn get_peers(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let _ = world;
    (
        tip(),
        caller(),
        prop_oneof![3 => 0usize..=5, 4 => 6usize..=40, 1 => 1999usize..=2002],
        1u64..=1_000,
        prop::bool::weighted(0.15),
        prop::option::of(0usize..=5),
    )
        .prop_map(|(tip, ip, n, seed, dropped, listed)| {
            let peers: Vec<Value> = (0..n)
                .map(|i| {
                    let at = u64::try_from(i).unwrap_or(0);
                    let address = if at % 9 == seed % 9 {
                        format!("2001:db8::{:x}", at + 1)
                    } else {
                        format!("10.{}.{}.{}", seed % 200, at >> 8, at & 255)
                    };
                    let port = if dropped && at % 5 == 2 {
                        -1
                    } else {
                        4_000 + i64::try_from(at * seed % 60_000).unwrap_or(0)
                    };
                    let ip = if listed == Some(i) {
                        ip.clone()
                    } else {
                        address
                    };
                    json!({"ip": ip, "port": port, "latency": (at * seed) % 6})
                })
                .collect();
            let request = get_peers_encode_request(&GetPeersRequest::default());
            let fields = json!({"peers": peers});
            (
                "p2p.controller",
                u64::from(tip),
                input(RouteId::GetPeers, &request, &ip, fields),
            )
        })
        .boxed()
}

fn get_common_blocks(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let world = world.clone();
    tip()
        .prop_flat_map(|t| (Just(t), store_shape(t)))
        .prop_flat_map(|(t, shape)| {
            (
                Just((t, shape)),
                caller(),
                prop::collection::vec(
                    (
                        prop_oneof![5 => FIRST..=TIP_TOP, 1 => Just(FIRST), 1 => Just(TIP_TOP)],
                        prop::bool::weighted(0.1),
                        prop::bool::weighted(0.15),
                    ),
                    0..=10,
                ),
            )
        })
        .prop_map(move |((t, (low, memory)), ip, ids)| {
            let ids: Vec<String> = ids
                .into_iter()
                .enumerate()
                .map(|(i, (height, capitals, unknown))| {
                    if unknown {
                        format!("{:064x}", i + 1)
                    } else if capitals {
                        world.block(height).id.to_uppercase()
                    } else {
                        world.block(height).id.clone()
                    }
                })
                .collect();
            let request = get_common_blocks_encode_request(&GetCommonBlocksRequest {
                ids,
                ..GetCommonBlocksRequest::default()
            });
            let fields = json!({"store": world.store(t, low, memory)});
            (
                "p2p.controller",
                u64::from(t),
                input(RouteId::GetCommonBlocks, &request, &ip, fields),
            )
        })
        .boxed()
}

fn get_status(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let world = world.clone();
    (
        tip(),
        caller(),
        prop_oneof![1 => 0i64..1_000, 1 => 4_000i64..5_000, 4 => 0i64..8_000],
        prop::collection::vec(0usize..4, 0..=4),
    )
        .prop_map(move |(t, ip, offset, registered)| {
            let request = get_status_encode_request(&GetStatusRequest::default());
            let network = &world.network;
            let config = json!({
                "version": "4.3.1",
                "network": {
                    "version": network.pub_key_hash, "name": network.name,
                    "nethash": network.nethash, "explorer": network.explorer,
                    "token": {"name": network.token, "symbol": network.symbol},
                },
                "plugins": {
                    "@solar-network/api": {"enabled": true, "port": 6003, "estimateTotalCount": true},
                    "@solar-network/webhooks": {"enabled": false, "port": 6004},
                },
            });
            let keys: Vec<&String> = registered.iter().map(|&i| &world.delegates[i].0).collect();
            let fields = json!({"status": {
                "tip": {"hex": world.block(t).hex},
                "now": world.epoch_ms + i64::from(t) * 8_000 + offset,
                "peerConfig": config, "registered": keys, "forgerKeys": [],
            }});
            (
                "p2p.controller",
                u64::from(t),
                input(RouteId::GetStatus, &request, &ip, fields),
            )
        })
        .boxed()
}

fn get_blocks(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let request = |last: u32, limit: Option<u32>, only: Option<bool>| {
        get_blocks_encode_request(&GetBlocksRequest {
            last_block_height: Some(last),
            block_limit: limit,
            headers_only: only,
            ..GetBlocksRequest::default()
        })
    };
    let limit = prop_oneof![3 => Just(None), 2 => Just(Some(400u32)), 3 => (1u32..=6).prop_map(Some), 1 => Just(Some(0u32))];
    let only = prop_oneof![3 => Just(None), 3 => Just(Some(false)), 2 => Just(Some(true))];
    let small = {
        let world = world.clone();
        tip()
            .prop_flat_map(|t| (Just(t), store_shape(t)))
            .prop_flat_map(move |(t, shape)| {
                let last =
                    prop_oneof![2 => Just(0), 1 => Just(t), 1 => Just(t + 1), 5 => 0..=t + 1];
                (
                    Just((t, shape)),
                    caller(),
                    last,
                    limit.clone(),
                    only.clone(),
                )
            })
            .prop_map(move |((t, (low, memory)), ip, last, limit, only)| {
                let fields = json!({"store": world.store(t, low, memory)});
                (
                    "p2p.controller",
                    u64::from(t),
                    input(RouteId::GetBlocks, &request(last, limit, only), &ip, fields),
                )
            })
    };
    let cut = {
        let world = world.clone();
        (
            caller(),
            prop_oneof![3 => Just(0u32), 1 => 1u32..=4, 1 => Just(SYNTHETIC_ROWS)],
            prop_oneof![3 => Just(None), 2 => Just(Some(400u32)), 1 => (1u32..=12).prop_map(Some)],
            prop_oneof![9 => Just(None), 1 => Just(Some(true))],
            prop_oneof![4 => Just(150u32), 1 => Just(120u32)],
        )
            .prop_map(move |(ip, last, limit, only, per_block)| {
                let mut fields = json!({"digest": true});
                fields["store"] = world.synthetic(per_block);
                (
                    "p2p.controller",
                    u64::from(LAST),
                    input(RouteId::GetBlocks, &request(last, limit, only), &ip, fields),
                )
            })
    };
    prop_oneof![7 => small, 3 => cut].boxed()
}

/// The transactions of `picks` from the chain, and their ids as the processor knows them.
fn picked(world: &World, picks: &[usize]) -> (Vec<Vec<u8>>, Vec<String>) {
    let all = &world.transactions;
    let list: Vec<Vec<u8>> = picks.iter().map(|i| all[i % all.len()].clone()).collect();
    let ids = list.iter().map(|bytes| sha_hex(bytes)).collect();
    (list, ids)
}

fn get_unconfirmed(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let world = world.clone();
    (
        tip(),
        caller(),
        prop::collection::vec(0usize..64, 0..=8),
        prop_oneof![1 => Just(None), 2 => Just(Some(false)), 2 => Just(Some(true))],
        prop::collection::vec(0usize..64, 0..=3),
        0u32..=40,
    )
        .prop_map(move |(t, ip, candidates, count_only, excluded, extra)| {
            let (list, _) = picked(&world, &candidates);
            let (_, ids) = picked(&world, &excluded);
            let exclude = ids
                .iter()
                .filter_map(|id| hex::decode_array::<32>(id).ok())
                .collect();
            let request = get_unconfirmed_transactions_encode_request(&UnconfirmedRequestInput {
                count_only,
                exclude,
                headers: None,
            });
            let hexes: Vec<String> = list.iter().map(|bytes| hex::encode(bytes)).collect();
            let size = hexes.len() as u32 + extra;
            let fields = json!({"pool": {"size": size, "candidates": hexes}});
            (
                "p2p.controller",
                u64::from(t),
                input(RouteId::GetUnconfirmedTransactions, &request, &ip, fields),
            )
        })
        .boxed()
}

fn post_transactions(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let world = world.clone();
    (
        tip(),
        caller(),
        prop::collection::vec(0usize..64, 0..=6),
        prop::collection::vec(0usize..64, 0..=2),
        prop::collection::vec(0usize..64, 0..=2),
    )
        .prop_map(move |(t, ip, posted, known, refused)| {
            let (list, _) = picked(&world, &posted);
            let request = post_transactions_encode_request(&PostTransactionsInput {
                transactions: list,
                headers: None,
            });
            let (_, known) = picked(&world, &known);
            let (_, refused) = picked(&world, &refused);
            let fields =
                json!({"pool": {"size": 0, "candidates": [], "known": known, "refuse": refused}});
            (
                "p2p.controller",
                u64::from(t),
                input(RouteId::PostTransactions, &request, &ip, fields),
            )
        })
        .boxed()
}

/// Which block a postBlock case posts.
#[derive(Debug, Clone, Copy)]
enum Posted {
    Next,
    Reduced,
    Skipping,
    Old,
    HeaderOnly,
    TooMany(u32),
}

fn post_block(world: &Arc<World>) -> BoxedStrategy<OpCase> {
    let world = world.clone();
    let kind = prop_oneof![
        8 => Just(Posted::Next),
        3 => Just(Posted::Reduced),
        2 => Just(Posted::Skipping),
        2 => Just(Posted::Old),
        1 => Just(Posted::HeaderOnly),
        1 => (151u32..=400).prop_map(Posted::TooMany),
    ];
    (
        prop_oneof![1 => Just(FIRST), 9 => FIRST..=TIP_TOP - 2],
        caller(),
        kind,
        prop::option::weighted(0.75, prop::option::weighted(0.9, 1u32..=60)),
        prop::bool::weighted(0.2),
        prop::bool::weighted(0.5),
        prop::option::of(FIRST..=TIP_TOP),
    )
        .prop_map(
            move |(t, ip, kind, registered, pinged, forged, downloaded)| {
                let (block, header) = match kind {
                    Posted::Reduced => (&world.reduced[(t + 1 - FIRST) as usize], false),
                    Posted::Skipping => (world.block(t + 2), false),
                    Posted::Old => (world.block(t), false),
                    Posted::HeaderOnly => (world.block(t + 1), true),
                    Posted::Next | Posted::TooMany(_) => (world.block(t + 1), false),
                };
                let mut bytes = hex::decode(if header {
                    &block.header_hex
                } else {
                    &block.hex
                })
                .unwrap_or_default();
                if let Posted::TooMany(count) = kind {
                    bytes[44..48].copy_from_slice(&count.to_le_bytes());
                }
                let request = post_block_encode_request(&PostBlockRequest {
                    block: Some(bytes),
                    ..PostBlockRequest::default()
                });
                let mut fields = json!({
                    "store": world.store(t, FIRST, 100),
                    "pinged": pinged, "alreadyForged": forged,
                });
                if let Some(rank) = registered {
                    let name = world
                        .delegates
                        .iter()
                        .find(|(key, _)| *key == block.generator)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_default();
                    let rank = rank.map_or(Value::Null, |rank| json!(rank));
                    fields["delegates"] =
                        json!([{"publicKey": block.generator, "username": name, "rank": rank}]);
                }
                if let Some(older) = downloaded.filter(|older| *older <= t) {
                    fields["lastDownloaded"] = json!(world.block(older).hex);
                }
                (
                    "p2p.controller",
                    u64::from(t),
                    input(RouteId::PostBlock, &request, &ip, fields),
                )
            },
        )
        .boxed()
}

/// The cases of P20. A context whose milestones cannot make the chain gives no case at all.
pub fn served_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let Some(world) = World::new(context).map(Arc::new) else {
        return Just(("p2p.controller", 2, json!({}))).boxed();
    };
    prop_oneof![
        get_peers(&world),
        get_common_blocks(&world),
        get_status(&world),
        get_blocks(&world),
        get_unconfirmed(&world),
        post_transactions(&world),
        post_block(&world),
    ]
    .boxed()
}

/// The public routes P20 must reach.
const PUBLIC: [RouteId; 7] = [
    RouteId::GetPeers,
    RouteId::GetCommonBlocks,
    RouteId::GetStatus,
    RouteId::GetBlocks,
    RouteId::PostBlock,
    RouteId::GetUnconfirmedTransactions,
    RouteId::PostTransactions,
];

/// The key `seen` counts the getBlocks cut under.
const CUT: &str = "P20 getBlocks cut";

fn route_key(route: RouteId) -> String {
    format!("P20 route {}", route.id())
}

/// Whether the getBlocks case `request` over a synthetic store of `per_block`-transaction rows
/// stopped at the cut: fewer blocks than the rows it asked for.
fn cut_short(input: &Value, expected: &Expected) -> bool {
    let (Some(synthetic), Expected::Output(output)) = (input["store"].get("synthetic"), expected)
    else {
        return false;
    };
    let Some(asked) = (input["request"].as_str())
        .and_then(|text| hex::decode(text).ok())
        .and_then(|bytes| get_blocks_decode_request(&bytes).ok())
    else {
        return false;
    };
    if asked.headers_only == Some(true) {
        return false;
    }
    let last = u64::from(asked.last_block_height.unwrap_or(0));
    let limit = match asked.block_limit {
        None | Some(0) => u64::from(DEFAULT_LIMIT),
        Some(limit) => u64::from(limit),
    };
    let from = synthetic["from"].as_u64().unwrap_or(0);
    let rows = from..from + synthetic["count"].as_u64().unwrap_or(0);
    let available = rows.filter(|h| *h > last && *h <= last + limit).count() as u64;
    output["digest"]["blocks"]
        .as_u64()
        .is_some_and(|sent| sent < available)
}

/// Count in `seen` what the case with `input`, answered `expected` by the reference, reached: its
/// route, and the getBlocks cut.
pub fn outcomes(input: &Value, expected: &Expected, seen: &mut BTreeMap<String, usize>) {
    if let Some(route) = PUBLIC
        .iter()
        .find(|r| input["route"].as_str() == Some(r.id()))
    {
        *seen.entry(route_key(*route)).or_insert(0) += 1;
    }
    if cut_short(input, expected) {
        *seen.entry(CUT.to_owned()).or_insert(0) += 1;
    }
}

/// P20's own check: each public route reached `served_reach` times and the getBlocks cut
/// `served_reach / 10` times; nothing is checked when `served_reach` is 0.
pub fn reach_checks(
    file: &str,
    seen: &BTreeMap<String, usize>,
    served_reach: u32,
) -> Vec<CaseResult> {
    if served_reach == 0 {
        return Vec::new();
    }
    let count = |key: &str| seen.get(key).copied().unwrap_or(0);
    let wanted = |n: u32| usize::try_from(n).unwrap_or(usize::MAX);
    let mut fewer: Vec<String> = PUBLIC
        .iter()
        .map(|route| (route_key(*route), wanted(served_reach)))
        .chain([(CUT.to_owned(), wanted(served_reach / 10))])
        .filter(|(key, minimum)| count(key) < *minimum)
        .map(|(key, minimum)| format!("{key} {} of {minimum}", count(&key)))
        .collect();
    fewer.sort();
    let outcome = if fewer.is_empty() {
        Outcome::Pass
    } else {
        Outcome::HarnessError(format!("fewer: {}", fewer.join(", ")))
    };
    vec![CaseResult {
        file: file.to_owned(),
        line: 0,
        op: "P20".into(),
        label: format!("P20 reached each public route {served_reach} times"),
        height: 0,
        outcome,
    }]
}
