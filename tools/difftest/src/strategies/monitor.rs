//! The cases of P16, the monitor: random peer sets with attested keys, ranks, heights and
//! verification results for `monitor.forkDecision`, and random height lists for
//! `monitor.networkHeight` (exact results).
//!
//! The fork cases mix common heights of different digit counts (9, 10, 99, 100, ...), where the
//! reference's sort of the heights as text and a numeric sort disagree, and peer sets around the
//! threshold of half the delegates plus one.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;

/// The public key of delegate `n` of the cases, as text: not a point of the curve, which the
/// decision never needs.
fn key(n: u32) -> String {
    format!("02{n:064x}")
}

/// The delegates of every fork case: keys 1 to 60 with their number as rank, keys 61 and 62 of
/// delegates without a rank. Keys 63 to 65 are no delegate's.
fn delegates() -> Value {
    let mut map = Map::new();
    for n in 1..=60 {
        map.insert(key(n), json!(n));
    }
    map.insert(key(61), Value::Null);
    map.insert(key(62), Value::Null);
    Value::Object(map)
}

/// `monitor.networkHeight` over up to 40 heights, unknown and zero ones among them.
fn height_lists() -> BoxedStrategy<OpCase> {
    let height = prop_oneof![
        1 => Just(Value::Null),
        1 => Just(json!(0)),
        4 => (1u32..=300).prop_map(|h| json!(h)),
        2 => any::<u32>().prop_map(|h| json!(h)),
    ];
    prop::collection::vec(height, 0..40)
        .prop_map(|heights| ("monitor.networkHeight", 2, json!({ "heights": heights })))
        .boxed()
}

/// `monitor.forkDecision` at our height, over up to 30 peers each attesting up to 4 keys.
fn fork_cases() -> BoxedStrategy<OpCase> {
    let ours = prop_oneof![3 => 1u32..=300, 2 => 1u32..=12_000, 1 => 4_900u32..=5_300];
    ours.prop_flat_map(|ours| {
        let texts = vec![1u32, 5, 9, 10, 50, 99, 100, 101, 500, 999, 1_000, 5_000];
        let common = prop_oneof![
            2 => Just(ours),
            3 => 0..=ours,
            2 => prop::sample::select(texts),
            1 => ours..=ours.saturating_add(20),
        ];
        let theirs = prop_oneof![
            2 => Just(ours),
            3 => ours.saturating_sub(20)..=ours.saturating_add(20),
        ];
        let verification = prop::option::weighted(0.8, (common, theirs)).prop_map(move |v| {
            v.map_or(Value::Null, |(common, theirs)| {
                json!({ "myHeight": ours, "theirHeight": theirs, "highestCommonHeight": common })
            })
        });
        let keys = prop::collection::vec((1u32..=65).prop_map(key), 0..=4);
        let height = prop_oneof![
            1 => Just(Value::Null),
            4 => (ours.saturating_sub(10)..=ours.saturating_add(10)).prop_map(|h| json!(h)),
        ];
        let peer = (height, keys, verification).prop_map(|(height, keys, verification)| {
            json!({ "height": height, "publicKeys": keys, "verification": verification })
        });
        (Just(ours), prop::collection::vec(peer, 0..30))
    })
    .prop_map(|(ours, peers)| {
        let input = json!({ "height": ours, "peers": peers, "delegates": delegates() });
        ("monitor.forkDecision", u64::from(ours), input)
    })
    .boxed()
}

/// The cases of P16.
pub fn monitor_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    prop_oneof![1 => height_lists(), 3 => fork_cases()].boxed()
}
