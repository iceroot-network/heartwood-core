//! The cases of P19, the forger's decisions: random rounds, network states (every status, tip
//! heights near the round's and near `u32::MAX`, quorum counts around the threshold, over-height
//! headers by the delegate's key and by others, with and without an id), pools with transactions
//! the block holds or its schema refuses, status headers, slot numbers and times left around the
//! 2,000 ms margin, and failing calls, for `forger.decide` (the action and the lines must be the
//! reference's).
//!
//! The forging delegate is delegate 1; delegates 1 to 5 are the round's candidates. The
//! over-height headers are real blocks: empty ones of delegates 1 and 2, a signed header of
//! delegate 1 that counts a transaction, one of delegate 1 whose signature does not verify, and
//! the two empty ones without their id.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use heartwood_crypto::blocks::{factory, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockHeader, KeyPair, Milestones, Transaction};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::{TxContext, TxSpec};
use crate::vectors::Expected;

/// The forger's paths that P19 must reach besides the reference's actions and refusals: a block
/// the reference cannot make (the height after `u32::MAX`, or a pool transaction that the block's
/// schema refuses), which ends the attempt as an error before the status request; and an
/// over-height header without an id, which the sort of the distinct headers reads.
pub const PATHS: [&str; 2] = [
    "decide path: a block that cannot be made",
    "decide path: an over-height header without an id",
];

/// The network byte of a pool transaction that the block's schema refuses.
const OTHER_NETWORK: u8 = 30;

/// Count the paths of [`PATHS`] that the reference took on `input` (`answer`). A block that
/// cannot be made counts when the input has one, the calls before the block answer, and the
/// reference's attempt errs. A header without an id counts when the network state has one and
/// the reference's attempt passed the status refusals to the sort of the distinct headers: it
/// forged, came late, found its slot taken, or refused for an own header or the quorum.
pub fn paths(input: &Value, answer: &Expected, seen: &mut BTreeMap<String, usize>) {
    let Expected::Output(output) = answer else {
        return;
    };
    let (action, reason) = (output["action"].as_str(), output["reason"].as_str());
    let state = &input["networkState"];
    let first = input["firstAttempt"] == Value::Bool(true);
    let before_block = |call: &str| {
        call == "networkState" || call == "roundSlot" || (first && call == "transactions")
    };
    let answered = !input["fail"]["call"].as_str().is_some_and(before_block);
    let height = state["nodeHeight"].as_u64();
    let other_network = |transaction: &Value| {
        let bytes = transaction.as_str().and_then(|t| hex::decode(t).ok());
        bytes.and_then(|b| b.get(2).copied()) == Some(OTHER_NETWORK)
    };
    let pool = input["pool"]["transactions"].as_array();
    let refused = first && pool.is_some_and(|list| list.iter().any(other_network));
    let unmakeable = height == Some(u64::from(u32::MAX)) || (height.is_some() && refused);
    let mut count = |path: &str| *seen.entry(path.to_owned()).or_insert(0) += 1;
    if unmakeable && state["lastBlockId"].is_string() && answered && action == Some("error") {
        count(PATHS[0]);
    }
    let headers = state["quorumDetails"]["overHeightBlockHeaders"].as_object();
    let without_id = headers.is_some_and(|h| h.values().any(|header| header.get("id").is_none()));
    let sorted = matches!(action, Some("forged" | "late" | "sameSlot"))
        || matches!(reason, Some("alreadyForged" | "quorum"));
    if without_id && sorted {
        count(PATHS[1]);
    }
}

/// Delegate `n`'s key pair.
fn keys(n: u32) -> KeyPair {
    KeyPair::from_passphrase(&format!("forger decision delegate {n}")).expect("a key pair")
}

fn public(n: u32) -> String {
    keys(n).public_key().to_compressed().to_hex()
}

/// Delegate `n`'s wallet as a round lists it.
fn wallet(n: u32) -> Value {
    json!({
        "address": format!("D{n}"),
        "publicKey": public(n),
        "balance": "0",
        "nonce": "0",
        "delegate": { "username": format!("genesis_{n}"), "voteBalance": "0", "rank": n },
    })
}

/// A header as a relay keeps a peer's.
fn header_json(header: &BlockHeader) -> Value {
    json!({
        "id": serialiser::block_id(header).to_hex(), "version": header.version,
        "timestamp": header.timestamp, "previousBlock": hex::encode(&header.previous_block),
        "height": header.height, "numberOfTransactions": header.number_of_transactions,
        "totalAmount": header.total_amount.to_string(), "totalFee": header.total_fee.to_string(),
        "reward": header.reward.to_string(), "payloadLength": header.payload_length,
        "payloadHash": hex::encode(&header.payload_hash),
        "generatorPublicKey": header.generator_public_key.to_hex(),
        "blockSignature": header.block_signature.to_hex(),
    })
}

/// The over-height headers the cases draw from: delegate 2's empty block, delegate 1's empty
/// block, delegate 1's signed header that counts a transaction, delegate 1's header with a
/// signature that does not verify, and the two empty blocks without their id.
static HEADERS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let milestones = &MILESTONES;
    let empty = |n: u32| {
        let draft = factory::BlockDraft {
            version: 0,
            timestamp: 800,
            height: 101,
            previous_block: [0x12; 32],
            number_of_transactions: 0,
            total_amount: 0,
            total_fee: 0,
            reward: 0,
            payload_length: 0,
            payload_hash: sha256(b""),
        };
        let block = factory::make(draft, Vec::new(), &keys(n), Aux::fixed([5; 32]), milestones);
        block.expect("a block").header().clone()
    };
    let mut counted = empty(1);
    counted.number_of_transactions = 1;
    counted.payload_length = 32;
    counted.payload_hash = sha256(&[1; 32]);
    let hash = serialiser::signing_hash(&counted);
    let key = keys(1);
    counted.block_signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Block,
        &hash,
        key.secret_key(),
        Aux::fixed([6; 32]),
    )
    .expect("a signature");
    let mut unsigned = header_json(&empty(1));
    unsigned["blockSignature"] = json!("00".repeat(64));
    let without_id = |n: u32| {
        let mut header = header_json(&empty(n));
        if let Some(fields) = header.as_object_mut() {
            fields.shift_remove("id");
        }
        header
    };
    vec![
        header_json(&empty(2)),
        header_json(&empty(1)),
        header_json(&counted),
        unsigned,
        without_id(2),
        without_id(1),
    ]
});

/// The pool's transactions the cases draw from, in hex: two transfers the forger's block holds,
/// and a transfer signed for another network, which the block's schema refuses, so that the
/// block cannot be made.
fn pool_transactions(context: &TxContext) -> Vec<String> {
    let params = context.milestones.at(2);
    let transfer = |n: u32| {
        let recipient = context.recipients[n as usize];
        let spec = TxSpec::Transfer(vec![(recipient, u64::from(n) * 100_000_000)]);
        spec.build(&keys(10 + n), None, 1, 10_000_000, params)
            .expect("a transfer")
    };
    let other = other_network(&transfer(3), &keys(13)).expect("a transfer of another network");
    vec![
        hex::encode(transfer(1).bytes()),
        hex::encode(transfer(2).bytes()),
        hex::encode(&other),
    ]
}

/// The bytes of `transaction` with the network byte [`OTHER_NETWORK`], signed again by `key`.
fn other_network(transaction: &Transaction, key: &KeyPair) -> Option<Vec<u8>> {
    let bytes = transaction.bytes();
    let mut unsigned = bytes.get(..bytes.len().checked_sub(64)?)?.to_vec();
    *unsigned.get_mut(2)? = OTHER_NETWORK;
    let signature = sig::sign(
        SchemeId::Secp256k1Bip340,
        SigningDomain::Transaction,
        &sha256(&unsigned),
        key.secret_key(),
        Aux::fixed([7; 32]),
    )
    .ok()?;
    unsigned.extend_from_slice(signature.as_bytes());
    Some(unsigned)
}

/// Milestones for the headers: any devnet file serves, since only the header's signature and
/// schema are read.
static MILESTONES: LazyLock<Milestones> = LazyLock::new(|| {
    let network = heartwood_crypto::Network {
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
    };
    let milestone = json!({
        "height": 1, "activeDelegates": 53, "blockTime": 8,
        "block": { "version": 0, "maxTransactions": 150, "maxPayload": 2_097_152 },
        "epoch": "2026-01-01T00:00:00.000Z", "reward": 0,
        "burn": { "feeBasisPoints": 9000, "txAmount": 2_000_000 }, "donations": {},
        "dynamicReward": { "enabled": false }, "transfer": { "maximum": 256, "minimum": 1 },
    });
    Milestones::load(&json!([milestone]).to_string(), &network).expect("the milestones load")
});

/// A round: its tip at `height`, its slot `slot`, the active delegates, and the forgers at the
/// current and next index (none when out of range). `forger` draws the current one: mostly the
/// forging delegate 1 (added to the active ones when missing), sometimes another index, rarely
/// none.
fn round(height: u32, slot: i64, mut active: Vec<u32>, forger: u8, next: usize) -> Value {
    let current = match forger {
        0 => active.len(),
        1..=6 => {
            if !active.contains(&1) {
                active.push(1);
            }
            active.iter().position(|n| *n == 1).unwrap_or(0)
        }
        _ => usize::from(forger) % active.len().max(1),
    };
    let pick = |index: usize| active.get(index).map(|n| wallet(*n));
    let mut round = json!({
        "allDelegates": (1..=6).map(wallet).collect::<Vec<_>>(),
        "canForge": true,
        "current": 1,
        "delegates": active.iter().map(|n| wallet(*n)).collect::<Vec<_>>(),
        "lastBlock": { "height": height, "id": format!("{height:064x}") },
        "reward": "500000000",
        "timestamp": slot * 8,
    });
    for (key, wallet) in [("currentForger", pick(current)), ("nextForger", pick(next))] {
        if let Some(wallet) = wallet {
            round[key] = wallet;
        }
    }
    round
}

/// The tip's height in a network state: the round's height with an offset, a height near
/// `u32::MAX` (the largest one has no next block), or none.
#[derive(Debug, Clone, Copy)]
enum Tip {
    Offset(i64),
    At(u32),
    Missing,
}

/// A network state's draw: the status, the tip's height, whether it has the tip's id, its
/// generator (a username, the delegate's key, or none), its slot offset, the quorum's counts,
/// and the over-height headers.
type StateDraw = (u8, Tip, bool, u8, Option<i64>, u32, u32, Vec<usize>);

fn network_state() -> BoxedStrategy<StateDraw> {
    (
        prop_oneof![12 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8), 1 => Just(4u8)],
        prop_oneof![
            8 => Just(Tip::Offset(0)),
            2 => (-3i64..=3).prop_map(Tip::Offset),
            1 => Just(Tip::Missing),
            1 => prop::sample::select(vec![u32::MAX - 1, u32::MAX]).prop_map(Tip::At),
        ],
        prop_oneof![12 => Just(true), 1 => Just(false)],
        0u8..=3,
        prop_oneof![3 => Just(None), 3 => Just(Some(0i64)), 2 => (-2i64..=2).prop_map(Some)],
        prop_oneof![3 => 0u32..=60, 2 => 24u32..=30, 2 => 50u32..=56],
        prop_oneof![3 => 0u32..=10, 2 => 20u32..=30, 1 => 0u32..=60],
        prop::collection::vec(0usize..6, 0..=3),
    )
        .boxed()
}

/// The cases of P19.
pub fn decision_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let transactions = pool_transactions(context);
    let private = keys(1).secret_key().to_hex().to_string();
    let fails = [
        "networkState",
        "roundSlot",
        "transactions",
        "status",
        "currentSlot",
        "lastBlockSlot",
        "blockSlot",
    ];
    let fail = prop_oneof![
        9 => Just(None),
        1 => (prop::sample::select(fails.to_vec()), any::<bool>()).prop_map(|(call, ready)| Some((call, ready))),
    ];
    let slots = (
        prop_oneof![5 => Just(0i64), 1 => Just(1i64), 1 => Just(-1i64)],
        prop_oneof![4 => Just(-1i64), 2 => Just(0i64), 1 => -3i64..=1],
        prop_oneof![5 => Just(0i64), 1 => Just(-1i64), 1 => Just(1i64)],
    );
    let time_left = prop_oneof![
        4 => 2_000i64..=8_000,
        2 => prop::sample::select(vec![1_999i64, 2_000, 2_001, 1, 0, -1]),
        2 => -9_000i64..=9_000,
    ];
    let active = prop::sample::subsequence(vec![1u32, 2, 3, 4, 5], 1..=5).prop_shuffle();
    let status = (
        prop_oneof![5 => Just(-1i64), 2 => Just(0i64), 1 => Just(-2i64)],
        prop_oneof![3 => Just(Some(2u32)), 2 => Just(Some(1u32)), 1 => Just(None)],
    );
    // Indexes of `transactions`: none, the block's own, or with the one of another network.
    let pool = (
        0u32..=20,
        prop_oneof![
            6 => Just(vec![]),
            2 => Just(vec![0usize]),
            1 => Just(vec![0, 1]),
            1 => Just(vec![2]),
            1 => Just(vec![1, 2]),
        ],
    );
    (
        (4u32..=300, 10i64..=5_000, active, 0u8..10, 0usize..=5),
        network_state(),
        (status, slots, time_left, any::<bool>(), pool, fail),
    )
        .prop_map(move |((height, slot, active, current, next), state, (status, slots, time_left, first, pool, fail))| {
            let (status_code, tip, has_id, generator, slot_offset, has, no, headers) = state;
            let mut network_state = json!({
                "status": status_code,
                "quorumDetails": {
                    "hasQuorum": has, "noQuorum": no, "overHeight": headers.len(),
                    "overHeightBlockHeaders": headers.iter().enumerate()
                        .map(|(i, h)| (format!("192.0.2.{}", i + 1), HEADERS[*h].clone()))
                        .collect::<serde_json::Map<_, _>>(),
                    "forked": 0, "differentSlot": 0, "forgingNotAllowed": 0,
                    "delegates": { "hasQuorum": ["genesis_2"], "noQuorum": [], "noResponse": ["genesis_9"] },
                },
            });
            let node_height = match tip {
                Tip::Offset(offset) => Some(i64::from(height) + offset),
                Tip::At(tip) => Some(i64::from(tip)),
                Tip::Missing => None,
            };
            if let Some(node_height) = node_height {
                network_state["nodeHeight"] = json!(node_height);
            }
            if has_id {
                network_state["lastBlockId"] = json!(format!("{height:064x}"));
            }
            match generator {
                0 => network_state["lastGenerator"] = json!("genesis_2"),
                1 => network_state["lastGenerator"] = json!(public(1)),
                _ => {}
            }
            if let Some(offset) = slot_offset {
                network_state["lastSlotNumber"] = json!(slot + offset);
            }
            let ((status_offset, generator), (current_offset, last_offset, block_offset)) = (status, slots);
            let mut header = json!({ "timestamp": (slot + status_offset) * 8 });
            if let Some(n) = generator {
                header["generatorPublicKey"] = json!(public(n));
            }
            let in_pool: Vec<&String> = pool.1.iter().map(|i| &transactions[*i]).collect();
            let last = slot + last_offset;
            let block = if block_offset == 0 { slot } else { last };
            let mut input = json!({
                "round": round(height, slot, active, current, next),
                "networkState": network_state,
                "status": header,
                "slots": [slot, slot + current_offset, last, block],
                "timeLeftInMs": time_left,
                "privateKey": private,
                "firstAttempt": first,
                "pool": { "poolSize": pool.0, "transactions": in_pool },
            });
            if let Some((call, ready)) = fail {
                let message = if ready { r#"{"error":"Unknown","message":"App is not ready"}"# } else { "Request timed out" };
                input["fail"] = json!({ "call": call, "message": message });
            }
            ("forger.decide", 2, input)
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::Transaction;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    fn context() -> Arc<TxContext> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/crypto/tests/vectors");
        let set = crate::vectors::VectorSet::load(&dir).unwrap();
        let devnet = crate::subjects::Devnet::from_vectors(&set)
            .unwrap()
            .unwrap();
        let text = crate::levels::l1::devnet_milestones(&set).unwrap();
        Arc::new(TxContext::new(devnet.network, text).unwrap())
    }

    #[test]
    fn the_cases_draw_both_paths_and_pool_transactions_the_forger_reads() {
        let context = context();
        let params = context.milestones.at(2);
        let mut runner = TestRunner::deterministic();
        let strategy = decision_cases(&context);
        let (mut top, mut refused, mut read, mut without_id) = (0, 0, 0, 0);
        for _ in 0..3_000 {
            let (_, _, input) = strategy.new_tree(&mut runner).unwrap().current();
            let state = &input["networkState"];
            if state["nodeHeight"] == json!(u32::MAX) {
                top += 1;
            }
            let pool = input["pool"]["transactions"].as_array().unwrap();
            for transaction in pool {
                let bytes = hex::decode(transaction.as_str().unwrap()).unwrap();
                let decoded = Transaction::decode_trusted(&bytes, params).unwrap();
                if decoded.data().network == OTHER_NETWORK {
                    refused += 1;
                } else {
                    assert_eq!(decoded.data().network, params.network());
                    read += 1;
                }
            }
            let headers = state["quorumDetails"]["overHeightBlockHeaders"]
                .as_object()
                .unwrap();
            if headers.values().any(|header| header.get("id").is_none()) {
                without_id += 1;
            }
        }
        assert!(top >= 60, "a height after which no block can follow: {top}");
        assert!(refused >= 60, "a transaction of another network: {refused}");
        assert!(read >= 300, "transactions the block holds: {read}");
        assert!(without_id >= 300, "a header without an id: {without_id}");
    }

    /// The paths `paths` counts for `input` and the reference's `action` and `reason`.
    fn counted(input: &Value, action: &str, reason: Option<&str>) -> Vec<String> {
        let answer = Expected::Output(json!({ "action": action, "reason": reason, "lines": [] }));
        let mut seen = BTreeMap::new();
        paths(input, &answer, &mut seen);
        seen.into_keys().collect()
    }

    #[test]
    fn paths_count_only_what_the_reference_reached() {
        let top = json!({
            "networkState": { "nodeHeight": u32::MAX, "lastBlockId": "00" },
            "firstAttempt": false,
        });
        assert_eq!(counted(&top, "error", None), [PATHS[0]]);
        assert!(counted(&top, "stop", None).is_empty(), "no error");
        let mut failed = top.clone();
        failed["fail"] = json!({ "call": "roundSlot", "message": "Request timed out" });
        assert!(counted(&failed, "error", None).is_empty(), "failed before");
        failed["fail"]["call"] = json!("transactions");
        assert_eq!(counted(&failed, "error", None), [PATHS[0]], "not asked");
        let mut below = top.clone();
        below["networkState"]["nodeHeight"] = json!(u32::MAX - 1);
        assert!(counted(&below, "error", None).is_empty());
        let mut unknown = top.clone();
        unknown["networkState"]
            .as_object_mut()
            .unwrap()
            .remove("lastBlockId");
        assert!(counted(&unknown, "error", None).is_empty());
        let other = format!("ff02{:02x}{}", OTHER_NETWORK, "00".repeat(8));
        let mut pool = below.clone();
        pool["pool"] = json!({ "poolSize": 1, "transactions": [other] });
        assert!(counted(&pool, "error", None).is_empty(), "not read");
        pool["firstAttempt"] = json!(true);
        assert_eq!(counted(&pool, "error", None), [PATHS[0]]);
        let header = json!({
            "networkState": {
                "quorumDetails": { "overHeightBlockHeaders": { "192.0.2.1": { "height": 101 } } },
            },
        });
        assert_eq!(counted(&header, "late", None), [PATHS[1]]);
        assert_eq!(counted(&header, "refused", Some("quorum")), [PATHS[1]]);
        assert!(counted(&header, "refused", Some("coldStart")).is_empty());
        assert!(counted(&header, "stop", None).is_empty());
        let mut with_id = header.clone();
        with_id["networkState"]["quorumDetails"]["overHeightBlockHeaders"]["192.0.2.1"]["id"] =
            json!("ab");
        assert!(counted(&with_id, "forged", None).is_empty());
    }
}
