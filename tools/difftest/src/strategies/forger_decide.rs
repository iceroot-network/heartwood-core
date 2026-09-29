//! The cases of P19, the forger's decisions: random rounds, network states (every status, quorum
//! counts around the threshold, over-height headers by the delegate's key and by others), status
//! headers, slot numbers and times left around the 2,000 ms margin, and failing calls, for
//! `forger.decide` (the action and the lines must be the reference's).
//!
//! The forging delegate is delegate 1; delegates 1 to 5 are the round's candidates. The
//! over-height headers are real blocks: empty ones of delegates 1 and 2, a signed header of
//! delegate 1 that counts a transaction, and one of delegate 1 whose signature does not verify.

use std::sync::{Arc, LazyLock};

use heartwood_crypto::blocks::{factory, serialiser};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::sig::{self, SchemeId, SigningDomain};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockHeader, KeyPair, Milestones};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;

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
/// block, delegate 1's signed header that counts a transaction, and delegate 1's header with a
/// signature that does not verify.
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
    vec![
        header_json(&empty(2)),
        header_json(&empty(1)),
        header_json(&counted),
        unsigned,
    ]
});

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

/// A network state's draw: the status, the tip's height offset (none: no height), whether it
/// has the tip's id, its generator (a username, the delegate's key, or none), its slot offset,
/// the quorum's counts, and the over-height headers.
type StateDraw = (u8, Option<i64>, bool, u8, Option<i64>, u32, u32, Vec<usize>);

fn network_state() -> BoxedStrategy<StateDraw> {
    (
        prop_oneof![12 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8), 1 => Just(4u8)],
        prop_oneof![8 => Just(Some(0i64)), 2 => (-3i64..=3).prop_map(Some), 1 => Just(None)],
        prop_oneof![12 => Just(true), 1 => Just(false)],
        0u8..=3,
        prop_oneof![3 => Just(None), 3 => Just(Some(0i64)), 2 => (-2i64..=2).prop_map(Some)],
        prop_oneof![3 => 0u32..=60, 2 => 24u32..=30, 2 => 50u32..=56],
        prop_oneof![3 => 0u32..=10, 2 => 20u32..=30, 1 => 0u32..=60],
        prop::collection::vec(0usize..4, 0..=3),
    )
        .boxed()
}

/// The cases of P19.
pub fn decision_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
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
    (
        (4u32..=300, 10i64..=5_000, active, 0u8..10, 0usize..=5),
        network_state(),
        (status, slots, time_left, any::<bool>(), 0u32..=20, fail),
    )
        .prop_map(move |((height, slot, active, current, next), state, (status, slots, time_left, first, pool, fail))| {
            let (status_code, height_offset, has_id, generator, slot_offset, has, no, headers) = state;
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
            if let Some(offset) = height_offset {
                network_state["nodeHeight"] = json!(i64::from(height) + offset);
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
                "pool": { "poolSize": pool, "transactions": [] },
            });
            if let Some((call, ready)) = fail {
                let message = if ready { r#"{"error":"Unknown","message":"App is not ready"}"# } else { "Request timed out" };
                input["fail"] = json!({ "call": call, "message": message });
            }
            ("forger.decide", 2, input)
        })
        .boxed()
}
