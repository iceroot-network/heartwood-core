//! The cases of P18, the quorum: random peer sets (heights, fast verifications, attested keys,
//! states), the forger's attested keys, round sets that differ from the ranking, ranks and
//! heights, for `quorum.analyze` (the answer's fields and `canForge` must be the reference's).
//!
//! Keys 0 to 69 are the cases' candidates; the first `n` of them are registered delegates, with
//! a random rank or none, and the round's set is drawn from them in any order. The relay's tip is
//! an empty block of a random generator, registered or not.

use std::sync::{Arc, LazyLock};

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, KeyPair, Milestones};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The candidate keys.
const KEYS: usize = 70;

/// Each candidate's passphrase, private key (hex) and public key (hex).
static CANDIDATES: LazyLock<Vec<(String, String, String)>> = LazyLock::new(|| {
    (0..KEYS)
        .map(|n| {
            let passphrase = format!("quorum case delegate {n}");
            let secret = hex::encode(&sha256(passphrase.as_bytes()));
            let keys = KeyPair::from_passphrase(&passphrase).expect("a key pair");
            (
                passphrase,
                secret,
                keys.public_key().to_compressed().to_hex(),
            )
        })
        .collect()
});

fn public(n: usize) -> String {
    CANDIDATES[n % KEYS].2.clone()
}

/// An empty block at `height`, in slot `height`, by candidate `generator`, as hex.
fn tip(height: u32, generator: usize, milestones: &Milestones) -> Option<String> {
    let keys = KeyPair::from_passphrase(&CANDIDATES[generator % KEYS].0).ok()?;
    let draft = BlockDraft {
        version: 0,
        timestamp: height.checked_mul(8)?,
        height,
        previous_block: [0; 32],
        number_of_transactions: 0,
        total_amount: 0,
        total_fee: 0,
        reward: 0,
        payload_length: 0,
        payload_hash: sha256(b""),
    };
    let block = factory::make(draft, Vec::new(), &keys, Aux::fixed([7; 32]), milestones).ok()?;
    Some(hex::encode(&block.encode()))
}

/// A peer's state header at `height`, its fields in the wire's order.
fn header(height: u32, generator: usize) -> Value {
    json!({
        "id": format!("{height:064x}"), "version": 0, "timestamp": height.saturating_mul(8),
        "height": height, "numberOfTransactions": 0, "totalAmount": "0", "totalFee": "0",
        "reward": "180000000", "payloadLength": 0, "generatorPublicKey": public(generator),
    })
}

/// A peer: keys drawn mostly from the first `registered` candidates, and a state around our
/// height and slot (at our height most often), and a fast verification (unforked most often).
fn peer(tip: u32, slot: i64, registered: usize) -> impl Strategy<Value = Value> {
    let delta = prop_oneof![6 => Just(0i64), 1 => -2i64..=-1, 1 => 1i64..=3];
    let state = prop::option::weighted(
        0.9,
        (delta, any::<bool>(), -1i64..=1, 0usize..KEYS).prop_map(move |(dh, allowed, ds, g)| {
            let height = u32::try_from(i64::from(tip) + dh).unwrap_or(tip);
            json!({ "height": height, "forgingAllowed": allowed, "currentSlot": slot + ds,
                    "header": header(height, g) })
        }),
    );
    let fast = prop_oneof![
        1 => Just(Value::Null),
        6 => Just(json!({ "myHeight": tip, "theirHeight": tip, "highestCommonHeight": tip })),
        1 => Just(json!({ "myHeight": tip, "theirHeight": tip + 2, "highestCommonHeight": tip.saturating_sub(3) })),
        1 => Just(json!({ "myHeight": tip, "theirHeight": tip })),
    ];
    let key = prop_oneof![5 => 0..registered.max(1), 1 => 0usize..KEYS].prop_map(public);
    let keys = prop::collection::vec(key, 0..=8);
    (keys, state, fast).prop_map(|(keys, state, fast)| {
        json!({ "publicKeys": keys, "state": state.unwrap_or(Value::Null), "fastVerification": fast })
    })
}

/// The cases of P18.
pub fn quorum_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let milestones = context.milestones.clone();
    let first = milestones.at(1);
    let epoch_ms = slots::epoch_unix_ms(first.epoch()).unwrap_or_default();
    let block_ms = i64::try_from(first.block_time()).unwrap_or(8) * 1_000;
    let registered = prop_oneof![1 => 0usize..=64, 3 => 30usize..=64];
    let shape = (2u32..=200, 0usize..KEYS, registered, 0i64..40, 0i64..1_000);
    shape
        .prop_flat_map(move |(height, generator, registered, later, ms)| {
            let slot = i64::from(height) + later;
            let now = epoch_ms + slot * block_ms + ms;
            let rank = prop::option::weighted(0.9, prop_oneof![4 => 1u32..=53, 1 => 54u32..=70]);
            let ranks = prop::collection::vec(rank, registered);
            let order = Just((0..registered).collect::<Vec<_>>()).prop_shuffle();
            let active = (order, 0usize..=registered.min(53))
                .prop_map(|(order, n)| order.into_iter().take(n).collect::<Vec<_>>());
            let peers = prop::collection::vec(peer(height, slot, registered), 0..=10);
            let declared = prop::collection::vec(prop_oneof![4 => 0..registered.max(1), 1 => 0usize..KEYS], 0..=8);
            let listed = prop::collection::vec(any::<bool>(), 8);
            let reach = prop_oneof![
                1 => Just(None),
                3 => (0u32..=8).prop_map(Some),
                1 => (0u32..=60).prop_map(Some),
            ];
            let tip = tip(height, generator, &milestones);
            (Just((height, now, tip)), ranks, active, peers, declared, listed, any::<u8>(), reach)
        })
        .prop_map(|((height, now, tip), ranks, active, peers, declared, listed, cold, reach)| {
            let delegates: Vec<Value> = (ranks.iter().enumerate())
                .map(|(n, rank)| json!({ "publicKey": public(n), "username": format!("d{n}"), "rank": rank }))
                .collect();
            let peers: Vec<Value> = (peers.into_iter().enumerate())
                .map(|(i, mut peer)| {
                    peer["ip"] = json!(format!("10.0.{}.{}", i / 250, i % 250 + 1));
                    peer
                })
                .collect();
            let forger_keys: Vec<String> = (declared.iter().zip(&listed))
                .filter(|(_, listed)| **listed)
                .map(|(&n, _)| public(n))
                .collect();
            let mut input = json!({
                "tip": { "hex": tip.unwrap_or_default() },
                "delegates": delegates,
                "active": active.iter().map(|&n| public(n)).collect::<Vec<_>>(),
                "peers": peers,
                "declared": declared.iter().map(|&n| CANDIDATES[n].1.clone()).collect::<Vec<_>>(),
                "forgerKeys": forger_keys,
                "now": now,
                "coldStart": cold < 26,
            });
            if let Some(reach) = reach {
                input["minimumDelegateReach"] = json!(reach);
            }
            ("quorum.analyze", u64::from(height), input)
        })
        .boxed()
}
