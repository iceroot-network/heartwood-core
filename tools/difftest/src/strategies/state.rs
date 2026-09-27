//! Inputs of the state's properties: the forging shuffle (P4), the delegate ranking (P5) and a
//! voter's shares (P6). Each case is an operation, a rule height and the operation's input.

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::MAX_SUPPLY;
use super::kernel::OpCase;

/// P4: `state.shuffle` for rounds 1 to `2^53 − 1` and 1 to 53 delegates.
pub fn shuffle_cases() -> BoxedStrategy<OpCase> {
    (
        prop_oneof![3 => 1u64..=(1 << 53) - 1, 2 => 1u64..=10_000, 1 => ((1u64 << 53) - 100)..=(1 << 53) - 1],
        1u64..=53,
    )
        .prop_map(|(round, count)| ("state.shuffle", 2u64, json!({"round": round, "count": count})))
        .boxed()
}

/// Valid public keys for the delegates of the ranking cases, derived once.
fn key_pool() -> &'static [String] {
    static POOL: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        (0..160)
            .map(|i| super::keys(&format!("l1 rank {i}")).public_key().to_hex())
            .collect()
    })
}

/// P5: `state.rank` for 1 to 80 delegates with many equal vote balances, resigned delegates (a
/// resignation type, 0 or 1), and now and then two delegates with the same key.
pub fn ranking_cases() -> BoxedStrategy<OpCase> {
    let delegate = (
        prop_oneof![3 => (0u64..=5).prop_map(|b| b * 1_000), 2 => 0u64..=MAX_SUPPLY, 1 => Just(0u64)],
        prop::bool::weighted(0.15),
    );
    (1usize..=80)
        .prop_flat_map(move |n| {
            (
                prop::collection::vec(delegate.clone(), n),
                prop::sample::subsequence(key_pool().to_vec(), n).prop_shuffle(),
            )
        })
        .prop_map(|(list, keys)| list.into_iter().zip(keys).map(|((balance, resigned), key)| (key, balance, resigned)).collect::<Vec<_>>())
        .prop_flat_map(|list| (Just(list), prop::bool::weighted(0.05), any::<usize>()))
        .prop_map(|(list, duplicate, pick)| {
            let mut keys: Vec<String> = list.iter().map(|(k, _, _)| k.clone()).collect();
            if duplicate && keys.len() > 1 {
                let from = pick % keys.len();
                let to = (from + 1) % keys.len();
                keys[to] = keys[from].clone();
            }
            let delegates: Vec<Value> = list
                .iter()
                .zip(keys)
                .enumerate()
                .map(|(i, ((_, balance, resigned), key))| {
                    let mut d = json!({"username": format!("rank{i}"), "publicKey": key, "voteBalance": balance.to_string()});
                    if *resigned {
                        // The resignation type: 0 temporary, 1 permanent.
                        d["resigned"] = json!(i % 2);
                    }
                    d
                })
                .collect();
            ("state.rank", 2u64, json!({"delegates": delegates}))
        })
        .boxed()
}

/// P6: `wallet.voteAmounts` for balances 0 to `2^63 − 1` and maps of 1 to 53 entries adding up
/// to 10,000 basis points, in any order.
pub fn vote_amount_cases() -> BoxedStrategy<OpCase> {
    let balance = prop_oneof![
        2 => 0u64..=1_000,
        2 => 0u64..=MAX_SUPPLY,
        1 => (MAX_SUPPLY - 1_000)..=MAX_SUPPLY,
        1 => prop::sample::select(vec![0u64, 1, 101, 9_999, 10_000, 10_001]),
    ];
    (
        balance,
        (1usize..=53).prop_flat_map(super::split),
        any::<u64>(),
    )
        .prop_map(|(balance, shares, salt)| {
            let votes: Vec<Value> = shares
                .iter()
                .enumerate()
                .map(|(i, bps)| {
                    json!([
                        format!("d{}", (salt as usize).wrapping_add(i * 7) % 997),
                        bps
                    ])
                })
                .collect();
            let mut seen = std::collections::BTreeSet::new();
            let votes: Vec<Value> = votes
                .into_iter()
                .filter(|v| seen.insert(v[0].as_str().unwrap_or_default().to_owned()))
                .collect();
            (votes, balance)
        })
        .prop_filter("names must be distinct", |(votes, _)| {
            votes
                .iter()
                .map(|v| v[1].as_u64().unwrap_or(0))
                .sum::<u64>()
                == 10_000
        })
        .prop_map(|(votes, balance)| {
            (
                "wallet.voteAmounts",
                2u64,
                json!({"balance": balance.to_string(), "votes": votes}),
            )
        })
        .boxed()
}
