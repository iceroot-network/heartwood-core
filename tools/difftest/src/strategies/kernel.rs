//! Inputs of the kernel's properties: rounds (P2), forging slots and block chaining (P3), and the
//! kernel's text formats (P8). Each case is an operation, a rule height and the operation's input.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value, json};

use super::heights;
use super::transactions::TxContext;

/// A case: the operation, the rule height, the input.
pub type OpCase = (&'static str, u64, Value);

/// P2: `round.calculate` and `round.isNew` at heights 1 to `2^32 − 1`, with 1 to 53 active
/// delegates (a milestone file with that count when it is not the devnet's).
pub fn round_cases(ctx: Arc<TxContext>) -> BoxedStrategy<OpCase> {
    (1u64..=53)
        .prop_flat_map(|n| (Just(n), heights(n), prop::bool::ANY))
        .prop_map(move |(n, height, is_new)| {
            let op = if is_new {
                "round.isNew"
            } else {
                "round.calculate"
            };
            let mut input = json!({"height": height});
            if n != ctx.milestones.at(1).active_delegates() {
                input["milestones"] = json!(ctx.milestones_with("activeDelegates", json!(n)));
            }
            (op, 2, input)
        })
        .boxed()
}

fn block_id() -> impl Strategy<Value = String> {
    any::<[u8; 32]>().prop_map(|b| heartwood_crypto::utils::hex::encode(&b))
}

/// P3: `forging.info` for times 0 to `2^32 − 1` and heights, and `block.isChained` for pairs of
/// headers: chained, and off by the previous id, the height or the slot.
pub fn slot_cases() -> BoxedStrategy<OpCase> {
    let info = (
        prop_oneof![3 => 0u32..=u32::MAX, 2 => 0u32..=100_000, 1 => (0u32..=1_000_000).prop_map(|s| s * 8)],
        heights(53),
    )
        .prop_map(|(timestamp, height)| ("forging.info", 2u64, json!({"timestamp": timestamp, "height": height})));
    let chained = (
        block_id(),
        1u32..=u32::MAX - 1,
        0u32..=u32::MAX - 100,
        prop_oneof![5 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8), 1 => Just(3u8)],
        0u32..=40,
        block_id(),
    )
        .prop_map(|(id, height, timestamp, defect, gap, other)| {
            let (previous_block, next_height, next_timestamp) = match defect {
                1 => (other, height + 1, timestamp.saturating_add(8 + gap)),
                2 => (id.clone(), height.saturating_add(2), timestamp.saturating_add(8 + gap)),
                3 => (id.clone(), height + 1, timestamp.saturating_add(gap % 8)),
                _ => (id.clone(), height + 1, timestamp.saturating_add(8 + gap)),
            };
            (
                "block.isChained",
                2u64,
                json!({
                    "previous": {"id": id, "height": height, "timestamp": timestamp},
                    "next": {"previousBlock": previous_block, "height": next_height, "timestamp": next_timestamp},
                }),
            )
        });
    prop_oneof![info, chained].boxed()
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..=1_000_000).prop_map(|n| json!(n)),
        (0u64..=(1 << 53)).prop_map(|n| json!(n)),
        (-1000i32..=1000, 1u32..=64).prop_map(|(a, b)| json!(f64::from(a) / f64::from(b))),
        "[ -~]{0,12}".prop_map(Value::String),
        "\\PC{0,6}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec(("[a-zA-Z0-9_$]{1,8}", inner), 0..6).prop_map(|pairs| {
                let mut map = Map::new();
                for (k, v) in pairs {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
    })
}

fn dotenv_line() -> impl Strategy<Value = String> {
    let key = "[A-Z_][A-Z0-9_]{0,10}";
    let value = prop_oneof![
        "[a-z0-9./:-]{0,12}",
        "[0-9]{1,6}",
        Just("true".to_owned()),
        Just("false".to_owned()),
        "\"[a-z =#]{0,10}\"",
        "'[a-z =#]{0,10}'",
        Just(String::new()),
        "[a-z]{1,5} #[a-z ]{0,6}",
    ];
    prop_oneof![
        6 => (key, prop_oneof![Just("="), Just(": "), Just(" = ")], value).prop_map(|(k, sep, v)| format!("{k}{sep}{v}")),
        1 => "#[ -~]{0,20}",
        1 => Just(String::new()),
        1 => "[ -~]{0,16}",
    ]
}

fn ip_pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        (0u8..=255, 0u8..=255, 0u8..=255, 0u8..=255)
            .prop_map(|(a, b, c, d)| format!("{a}.{b}.{c}.{d}")),
        (0u8..=3, 0u8..=3).prop_map(|(a, b)| format!("192.168.{a}.{b}")),
        (0u8..=3).prop_map(|a| format!("192.168.{a}.*")),
        Just("127.0.0.*".to_owned()),
        Just("*".to_owned()),
        Just("10.*.*.*".to_owned()),
        Just("::1".to_owned()),
        Just("::ffff:127.0.0.1".to_owned()),
        Just("2001:db8::*".to_owned()),
        Just("2001:0db8:0000:0000:0000:0000:0000:0001".to_owned()),
        Just("192.168.[0-1].1".to_owned()),
    ]
}

fn ip_address() -> impl Strategy<Value = String> {
    prop_oneof![
        (0u8..=3, 0u8..=3).prop_map(|(a, b)| format!("192.168.{a}.{b}")),
        Just("127.0.0.1".to_owned()),
        Just("::1".to_owned()),
        Just("::ffff:127.0.0.1".to_owned()),
        Just("2001:db8::1".to_owned()),
        Just("10.1.2.3".to_owned()),
        (0u8..=255, 0u8..=255, 0u8..=255, 0u8..=255)
            .prop_map(|(a, b, c, d)| format!("{a}.{b}.{c}.{d}")),
        Just("not an address".to_owned()),
    ]
}

/// P8: `kernel.stringify` of random JSON, `kernel.dotenv` of random `.env` text, and
/// `kernel.ip` of random lists and addresses.
pub fn kernel_text_cases() -> BoxedStrategy<OpCase> {
    prop_oneof![
        json_value().prop_map(|value| ("kernel.stringify", 2u64, json!({"value": value}))),
        prop::collection::vec(dotenv_line(), 0..12).prop_map(|lines| (
            "kernel.dotenv",
            2u64,
            json!({"text": lines.join("\n")})
        )),
        (prop::collection::vec(ip_pattern(), 0..6), ip_address()).prop_map(|(list, ip)| (
            "kernel.ip",
            2u64,
            json!({"list": list, "ip": ip})
        )),
    ]
    .boxed()
}
