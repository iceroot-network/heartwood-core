//! P23, the query grammar: random raw queries over every route's schema that the query target
//! reaches (its 27 routes: the served ones, the chain routes and the listings; nested keys to the
//! grammar's 128 levels, comma lists, holes, casts, `%` patterns, percent-encoding and bytes beyond
//! ASCII, repeated keys, the paging keys, the listings' criteria and orders, the fee statistics'
//! `days`), for `api.fuzzQuery`: the route's choice byte and the raw query through the grammar,
//! the route's validation and the pagination.
//!
//! A query is a list of pairs joined by `&`; a key is a name, sometimes nested by dots (to a
//! depth of 130, past the grammar's limit) or bracketed, and a value a plain token, a number, a
//! comma list, a percent pattern or percent-encoded text. A few queries carry raw bytes beyond
//! ASCII, some of which are not UTF-8.

use std::sync::Arc;

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::json;

use super::kernel::OpCase;
use super::transactions::TxContext;

/// Names the routes' criteria and the paging use, and some they do not.
const NAMES: [&str; 30] = [
    "page",
    "limit",
    "offset",
    "orderBy",
    "transform",
    "days",
    "id",
    "height",
    "timestamp",
    "senderId",
    "recipientId",
    "type",
    "typeGroup",
    "fee",
    "amount",
    "asset",
    "vendorField",
    "nonce",
    "version",
    "username",
    "balance",
    "publicKey",
    "address",
    "attributes",
    "generatorPublicKey",
    "votingFor",
    "isResigned",
    "a",
    "z",
    "__proto__",
];

/// Suffixes of a nested criterion.
const PARTS: [&str; 12] = [
    "from",
    "to",
    "asset",
    "payments",
    "recipientId",
    "0",
    "x",
    "constructor",
    "a",
    "delegate",
    "username",
    "*",
];

fn name() -> impl Strategy<Value = String> {
    let plain = prop::sample::select(NAMES.to_vec()).prop_map(str::to_owned);
    let nested = (
        prop::sample::select(NAMES.to_vec()),
        prop::collection::vec(prop::sample::select(PARTS.to_vec()), 1..=4),
    )
        .prop_map(|(head, tail)| format!("{head}.{}", tail.join(".")));
    let deep = (prop::sample::select(NAMES.to_vec()), 120usize..=130)
        .prop_map(|(head, depth)| format!("{head}{}", ".k".repeat(depth)));
    let bracketed = prop::sample::select(NAMES.to_vec()).prop_map(|head| format!("{head}[]"));
    let random = "[a-zA-Z0-9_.%]{0,8}";
    prop_oneof![
        12 => plain,
        4 => nested,
        1 => deep,
        1 => bracketed,
        2 => random,
    ]
}

fn token() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[0-9]{1,3}",
        2 => "[a-z0-9_]{1,10}",
        2 => (-5i32..40).prop_map(|n| n.to_string()),
        1 => prop::sample::select(vec![
            "", "0", "1", "30", "31", "07", "+7", "-1", "1.5", "1e1", "1e400", " 7", "7 ", "NaN",
            "Infinity", "true", "false", "null", "undefined", "9007199254740991",
            "9007199254740992", "18446744073709551616", "asc", "desc", "%20", "%", "%%", "%zz",
            "genesis_1", "balance:desc", "rank:asc", "username:desc",
            "attributes.delegate.voteBalance:desc",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()),
        1 => "[%][0-9A-Fa-f]{2}[a-z]{0,3}",
        1 => "[a-z]{1,4}[%_][a-z]{0,4}",
        1 => "\\PC{0,6}",
    ]
}

fn value() -> impl Strategy<Value = String> {
    let list = prop::collection::vec(token(), 1..=5).prop_map(|items| items.join(","));
    let holes = prop::collection::vec(prop_oneof![token(), Just(String::new())], 2..=5)
        .prop_map(|items| items.join(","));
    prop_oneof![
        8 => token(),
        3 => list,
        1 => holes,
        1 => token().prop_map(|text| format!("%{text}%")),
    ]
}

fn pair() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => (name(), value()).prop_map(|(name, value)| format!("{name}={value}")),
        1 => name(),
        1 => value().prop_map(|value| format!("={value}")),
        1 => Just(String::new()),
    ]
}

/// The bytes of a query: its pairs joined by `&`, sometimes with raw bytes beyond ASCII (valid
/// UTF-8 or not) appended to one of them.
fn query() -> impl Strategy<Value = Vec<u8>> {
    let pairs = prop::collection::vec(pair(), 0..=6).prop_map(|pairs| pairs.join("&"));
    let raw = prop_oneof![
        9 => Just(Vec::new()),
        1 => prop::collection::vec(0x80u8..=0xff, 1..=4),
    ];
    (pairs, raw).prop_map(|(text, bytes)| {
        let mut out = text.into_bytes();
        out.extend(bytes);
        out
    })
}

/// The cases of P23.
pub fn query_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    // The route's choice byte: the 27 routes of the target, and bytes past them, which wrap.
    let choice = prop_oneof![
        4 => 0u8..=10,
        6 => 11u8..=26,
        2 => Just(9u8),
        1 => any::<u8>(),
    ];
    (choice, query())
        .prop_map(|(choice, query)| {
            let mut bytes = vec![choice];
            bytes.extend(query);
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            ("api.fuzzQuery", 2u64, json!({ "hex": hex }))
        })
        .boxed()
}
