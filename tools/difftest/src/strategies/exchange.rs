//! The cases of P15, the exchange: random `ClientRequest` values for `p2p.validateRequest` (a
//! reference server must accept every one whose version passes its gate, and the bytes must be
//! the reference client's), and random replies of the six routes other than getStatus for
//! `wire.checkReply` (the verdict and the value must be the reference's).
//!
//! The replies are drawn field by field around the schemas' edges (present and absent fields,
//! addresses of both formats and neither, heights 0 and 1, hex of the wrong length or case,
//! amounts at 2^63 and beyond, more transactions than a small limit), written with the wire
//! crate's encoders, and a quarter of them then have a byte changed, cut or added.

use std::sync::Arc;

use heartwood_crypto::utils::hex;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::{peer, transactions};
use heartwood_p2p_wire::messages::Proto;
use heartwood_p2p_wire::messages::blocks::{BlockHeader, PostBlockResponse};
use heartwood_p2p_wire::messages::peer::{Common, GetCommonBlocksResponse, Peer};
use heartwood_p2p_wire::messages::shared::Headers;
use heartwood_p2p_wire::messages::transactions::GetUnconfirmedTransactionsResponse;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Value, json};

use super::kernel::OpCase;
use super::transactions::TxContext;

/// Versions for the request headers: the devnet's floor is `>=4.3.0` with prereleases, so some
/// pass and some do not.
const VERSIONS: [&str; 10] = [
    "4.3.1-heartwood.0.1.0",
    "4.3.1",
    "4.3.0",
    "v4.3.2",
    "4.9.0+build.1",
    "5.0.0-rc.1",
    "4.2.9",
    "4.3.0-next.1",
    "3.2.0",
    "4.3",
];

/// The cases of P15.
pub fn exchange_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    prop_oneof![2 => request_case(), 3 => reply_case()].boxed()
}

fn hex_of(bytes: impl Strategy<Value = Vec<u8>>) -> impl Strategy<Value = String> {
    bytes.prop_map(|b| hex::encode(&b))
}

fn id() -> impl Strategy<Value = String> {
    hex_of(prop::collection::vec(any::<u8>(), 32))
}

/// A request the client writes, as the oracle's payload.
fn request_case() -> impl Strategy<Value = OpCase> {
    let headers = (prop::sample::select(&VERSIONS[..]), 0u32..=65_535)
        .prop_map(|(version, port)| json!({ "version": version, "port": port }));
    let payload = prop_oneof![
        Just(("getStatus", json!({}))),
        Just(("getPeers", json!({}))),
        prop::collection::vec(id(), 1..=10)
            .prop_map(|ids| ("getCommonBlocks", json!({ "ids": ids }))),
        (1u32..=u32::MAX, 1u32..=400, any::<bool>()).prop_map(|(height, limit, only)| {
            let mut payload =
                json!({ "lastBlockHeight": height, "blockLimit": limit, "serialised": true });
            if only {
                payload["headersOnly"] = json!(true);
            }
            ("getBlocks", payload)
        }),
        prop::collection::vec(id(), 0..=40)
            .prop_map(|ids| ("getUnconfirmedTransactions", json!({ "exclude": ids }))),
        hex_of(prop::collection::vec(any::<u8>(), 0..=400))
            .prop_map(|block| ("postBlock", json!({ "block": block }))),
        prop::collection::vec(hex_of(prop::collection::vec(any::<u8>(), 1..=120)), 1..=40)
            .prop_map(|txs| ("postTransactions", json!({ "transactions": txs }))),
    ];
    (payload, headers).prop_map(|((route, mut payload), headers)| {
        payload["headers"] = headers;
        (
            "p2p.validateRequest",
            2,
            json!({ "route": route, "payload": payload }),
        )
    })
}

/// Text around the hex rules: 64-digit hex in either case, one digit short or long, not hex.
fn hex_text(length: usize) -> impl Strategy<Value = String> {
    let digits =
        prop::collection::vec(prop::sample::select(&b"0123456789abcdefABCDEF"[..]), length);
    let text = digits.prop_map(|d| String::from_utf8(d).unwrap_or_default());
    prop_oneof![
        6 => text.clone(),
        1 => text.clone().prop_map(|t| t.chars().skip(1).collect()),
        1 => text.clone().prop_map(|t| format!("{t}0")),
        1 => text.prop_map(|t| format!("g{}", t.chars().skip(1).collect::<String>())),
        1 => Just(String::new()),
    ]
}

fn maybe<T: std::fmt::Debug + Clone + 'static>(
    value: impl Strategy<Value = T>,
) -> impl Strategy<Value = Option<T>> {
    prop::option::weighted(0.9, value)
}

fn address() -> impl Strategy<Value = String> {
    let part = prop::sample::select(
        &[
            "0", "1", "10", "255", "256", "001", "01", "ffff", "FFFF", "fe80", "db8", "12345", "",
            "g",
        ][..],
    );
    let v4 = prop::collection::vec(part.clone(), 1..=5).prop_map(|p| p.join("."));
    let v6 = (
        prop::collection::vec(part, 1..=9),
        prop::sample::select(&[":", ":", "::"][..]),
    )
        .prop_map(|(p, sep)| p.join(sep));
    let around = prop::sample::select(&["", "", "", " ", "%eth0", "%", "%a\n"][..]);
    (
        prop_oneof![
            v4,
            v6,
            Just("2001:db8::1".to_owned()),
            Just("192.0.2.1".to_owned())
        ],
        around,
    )
        .prop_map(|(ip, suffix)| format!("{ip}{suffix}"))
}

/// Amounts at the header schema's edges: 0, a small one, -1, 2^63 - 1, 2^63 and one of 101 bits.
const AMOUNT_EDGES: [i128; 6] = [0, 5, -1, i64::MAX as i128, i64::MAX as i128 + 1, 1 << 100];

fn amount() -> impl Strategy<Value = Amount> {
    prop_oneof![
        3 => (0u64..=10_000_000_000).prop_map(Amount::from),
        2 => prop::sample::select(&AMOUNT_EDGES[..]).prop_map(Amount::new),
    ]
}

fn listed_block() -> impl Strategy<Value = ListedBlock> {
    let key = prop_oneof![
        4 => hex_text(64).prop_map(|t| format!("02{t}")),
        1 => hex_text(66),
    ];
    let height = prop_oneof![3 => 2u32..=10_000, 1 => Just(1u32), 1 => Just(0u32)];
    let previous = prop_oneof![4 => hex_text(64), 1 => prop::sample::select(&["", "0", "0x0", "00"][..]).prop_map(str::to_owned)];
    (
        (
            maybe(hex_text(64)),
            maybe(0u32..=100),
            maybe(previous),
            maybe(height),
        ),
        (maybe(hex_text(64)), maybe(key), maybe(hex_text(128))),
        (amount(), amount(), amount()),
        prop::collection::vec(prop::collection::vec(any::<u8>(), 1..=8), 0..=2),
    )
        .prop_map(
            |(
                (id, timestamp, previous_block, height),
                (payload_hash, generator_public_key, block_signature),
                (total_amount, total_fee, reward),
                transactions,
            )| ListedBlock {
                header: BlockHeader {
                    id,
                    version: Some(0),
                    timestamp,
                    previous_block,
                    height,
                    number_of_transactions: Some(0),
                    payload_length: Some(0),
                    payload_hash,
                    generator_public_key,
                    block_signature,
                    ..BlockHeader::default()
                },
                total_amount,
                total_fee,
                reward,
                transactions,
            },
        )
}

/// A reply of one of the six routes, written with the wire crate's encoders.
fn reply() -> impl Strategy<Value = (&'static str, Vec<u8>, Option<u64>)> {
    let peers = prop::collection::vec(
        (
            maybe(address()),
            maybe(prop_oneof![5 => 0u32..=65_535, 1 => 65_536u32..=u32::MAX]),
        ),
        0..=4,
    )
    .prop_map(|entries| {
        let peers: Vec<Peer> = (entries.into_iter())
            .map(|(ip, port)| Peer {
                ip,
                port,
                ..Peer::default()
            })
            .collect();
        ("getPeers", peer::get_peers_encode_response(&peers), None)
    });
    let common = maybe((
        maybe(prop_oneof![3 => 1u32..=100_000, 1 => Just(0u32)]),
        maybe(hex_text(64)),
    ))
    .prop_map(|common| {
        let reply = GetCommonBlocksResponse {
            common: common.map(|(height, id)| Common {
                height,
                id,
                ..Common::default()
            }),
            ..GetCommonBlocksResponse::default()
        };
        ("getCommonBlocks", reply.encode(), None)
    });
    let blocks = prop::collection::vec(listed_block(), 0..=3).prop_map(|blocks| {
        (
            "getBlocks",
            blocks::get_blocks_encode_response(&blocks),
            None,
        )
    });
    let post_block = (
        maybe(any::<bool>()),
        maybe(prop_oneof![3 => 1u32..=1_000_000, 1 => Just(0u32)]),
    )
        .prop_map(|(status, height)| {
            let reply = PostBlockResponse {
                status,
                height,
                ..PostBlockResponse::default()
            };
            ("postBlock", reply.encode(), None)
        });
    let unconfirmed = (maybe(any::<u32>()), 0usize..=8, any::<bool>(), 0u64..=6).prop_map(
        |(pool_size, count, headers, max)| {
            let reply = transactions::WithTransactions {
                message: GetUnconfirmedTransactionsResponse {
                    pool_size,
                    headers: headers.then(|| Headers {
                        version: Some("4.3.1".into()),
                        port: Some(4002),
                        ..Headers::default()
                    }),
                    ..GetUnconfirmedTransactionsResponse::default()
                },
                transactions: vec![vec![0xff, 0x01]; count],
            };
            (
                "getUnconfirmedTransactions",
                transactions::get_unconfirmed_transactions_encode_response(&reply),
                Some(max),
            )
        },
    );
    let post_transactions = prop::collection::vec(hex_text(64), 0..=3).prop_map(|accept| {
        (
            "postTransactions",
            transactions::post_transactions_encode_response(&accept),
            None,
        )
    });
    prop_oneof![
        peers,
        common,
        blocks,
        post_block,
        unconfirmed,
        post_transactions
    ]
}

/// A reply, a quarter of them with one byte changed, cut or added.
fn reply_case() -> impl Strategy<Value = OpCase> {
    (reply(), 0u8..=15, any::<prop::sample::Index>(), any::<u8>()).prop_map(
        |((route, mut bytes, max), mutation, at, byte)| {
            match mutation {
                0 if !bytes.is_empty() => {
                    let at = at.index(bytes.len());
                    bytes[at] ^= byte | 1;
                }
                1 if !bytes.is_empty() => bytes.truncate(at.index(bytes.len())),
                2 => bytes.insert(at.index(bytes.len() + 1), byte),
                3 => bytes.push(byte),
                _ => {}
            }
            let mut input = json!({ "route": route, "hex": hex::encode(&bytes) });
            if let Some(max) = max {
                input["maxTransactions"] = Value::from(max);
            }
            ("wire.checkReply", 2, input)
        },
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use heartwood_p2p_wire::replies::{self, ROUTES};
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    use super::*;

    /// The reply check's verdict on a case: accepted, the part refused, or the decoder's refusal.
    fn outcome(input: &Value) -> String {
        let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
        let route = ROUTES
            .into_iter()
            .find(|r| r.name() == input["route"])
            .unwrap();
        let checked = match route.name() {
            "getPeers" => {
                peer::get_peers_decode_response(&bytes).map(|p| replies::check_peers(&p).map(drop))
            }
            "getCommonBlocks" => peer::get_common_blocks_decode_response(&bytes)
                .map(|r| replies::check_common_blocks(&r).map(drop)),
            "getBlocks" => blocks::get_blocks_decode_response(&bytes)
                .map(|mut b| replies::check_blocks(&mut b)),
            "postBlock" => blocks::post_block_decode_response(&bytes)
                .map(|r| replies::check_post_block(&r).map(drop)),
            "getUnconfirmedTransactions" => {
                transactions::get_unconfirmed_transactions_decode_response(&bytes)
                    .map(|r| replies::check_unconfirmed(&r, 3).map(drop))
            }
            _ => transactions::post_transactions_decode_response(&bytes)
                .map(|a| replies::check_post_transactions(&a)),
        };
        match checked {
            Ok(Ok(())) => format!("{} accepted", route.name()),
            Ok(Err(part)) => format!("{} refused {part:?}", route.name()),
            Err(_) => format!("{} not decoded", route.name()),
        }
    }

    #[test]
    fn the_replies_reach_every_route_s_verdicts() {
        let mut runner = TestRunner::deterministic();
        let strategy = reply_case();
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for _ in 0..6_000 {
            let (_, _, input) = strategy.new_tree(&mut runner).unwrap().current();
            *seen.entry(outcome(&input)).or_default() += 1;
        }
        for wanted in [
            "getPeers accepted",
            "getPeers refused Peer",
            "getCommonBlocks accepted",
            "getCommonBlocks refused Common",
            "getBlocks accepted",
            "getBlocks refused Header",
            "getBlocks not decoded",
            "postBlock accepted",
            "postBlock refused PostBlock",
            "getUnconfirmedTransactions accepted",
            "getUnconfirmedTransactions refused Unconfirmed",
            "postTransactions accepted",
        ] {
            assert!(
                seen.get(wanted).is_some_and(|n| *n >= 10),
                "{wanted}: {seen:?}"
            );
        }
    }

    #[test]
    fn the_requests_cover_every_route() {
        let mut runner = TestRunner::deterministic();
        let strategy = request_case();
        let mut routes: BTreeMap<String, usize> = BTreeMap::new();
        for _ in 0..2_000 {
            let (_, _, input) = strategy.new_tree(&mut runner).unwrap().current();
            *routes
                .entry(input["route"].as_str().unwrap().to_owned())
                .or_default() += 1;
        }
        assert_eq!(routes.len(), 7, "{routes:?}");
        assert!(routes.values().all(|n| *n >= 100), "{routes:?}");
    }
}
