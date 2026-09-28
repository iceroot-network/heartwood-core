//! The block and transaction codecs: the list edges and caps, the amounts, the keys the codecs set,
//! and every record of the wire vectors (class V13) for their routes, encoded and decoded, with
//! the reference's bytes, values, key order and error classes. The records are read as
//! `common::records` says; a `wire.decodeSelected` record whose first byte selects one of these
//! codecs is checked as that codec's decode record.

mod common;

use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::codecs::transactions::{
    self, PostTransactionsInput, UnconfirmedRequestInput, WithTransactions,
};
use heartwood_p2p_wire::list::{
    self, MAX_BLOCK_TRANSACTIONS, MAX_BLOCKS, MAX_EXCLUSIONS, MAX_TRANSACTIONS,
};
use heartwood_p2p_wire::messages::blocks::{BlockHeader, PostBlockRequest, PostBlockResponse};
use heartwood_p2p_wire::messages::shared::Headers;
use heartwood_p2p_wire::messages::transactions::GetUnconfirmedTransactionsResponse;
use heartwood_p2p_wire::{Amount, DecodeError, Proto};
use serde_json::{Map, Value, json};

// Protobuf bytes for crafted payloads.

fn field(number: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint(u64::from(number << 3 | 2));
    out.extend(varint(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

fn uint(number: u32, value: u64) -> Vec<u8> {
    let mut out = varint(u64::from(number << 3));
    out.extend(varint(value));
    out
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// `n` items of one byte each: the item's index, modulo 256.
fn items(n: usize) -> Vec<Vec<u8>> {
    (0..n).map(|i| vec![i as u8]).collect()
}

/// A getBlocks response of one block with height 2 and this `transactions` field.
fn block_with_transactions(transactions: &[u8]) -> Vec<u8> {
    list::encode(&[[uint(7, 2), field(16, transactions)].concat()])
}

#[test]
fn a_getblocks_response_holds_at_most_401_blocks() {
    for n in [0, 1, MAX_BLOCKS, MAX_BLOCKS + 1, MAX_BLOCKS + 2] {
        let payload = list::encode(&(0..n as u64).map(|i| uint(7, i + 1)).collect::<Vec<_>>());
        let listed = blocks::get_blocks_decode_response(&payload).unwrap();
        assert_eq!(listed.len(), n.min(MAX_BLOCKS + 1), "{n} blocks");
        for (i, block) in listed.iter().enumerate() {
            assert_eq!(block.header.height, Some(i as u32 + 1));
            assert_eq!(block.total_amount, Amount::ZERO);
            assert!(block.transactions.is_empty());
        }
    }
}

#[test]
fn a_listed_block_holds_at_most_501_transactions() {
    for n in [
        0,
        1,
        MAX_BLOCK_TRANSACTIONS,
        MAX_BLOCK_TRANSACTIONS + 1,
        MAX_BLOCK_TRANSACTIONS + 2,
    ] {
        let payload = block_with_transactions(&list::encode(&items(n)));
        let listed = blocks::get_blocks_decode_response(&payload).unwrap();
        assert_eq!(
            listed[0].transactions,
            items(n.min(MAX_BLOCK_TRANSACTIONS + 1)),
            "{n}"
        );
    }
}

#[test]
fn transaction_lists_hold_at_most_1001_transactions() {
    for n in [
        0,
        1,
        MAX_TRANSACTIONS,
        MAX_TRANSACTIONS + 1,
        MAX_TRANSACTIONS + 2,
    ] {
        let list = list::encode(&items(n));
        let posted = transactions::post_transactions_decode_request(&field(1, &list)).unwrap();
        assert_eq!(
            posted.transactions,
            items(n.min(MAX_TRANSACTIONS + 1)),
            "{n}"
        );
        let pool =
            transactions::get_unconfirmed_transactions_decode_response(&field(2, &list)).unwrap();
        assert_eq!(pool.transactions, posted.transactions);
    }
}

#[test]
fn an_exclude_list_holds_at_most_15001_ids() {
    let id = |i: usize| -> [u8; 32] {
        let mut id = [0; 32];
        id[28..].copy_from_slice(&(i as u32).to_be_bytes());
        id
    };
    for n in [0, 1, MAX_EXCLUSIONS, MAX_EXCLUSIONS + 1, MAX_EXCLUSIONS + 2] {
        let ids: Vec<[u8; 32]> = (0..n).map(id).collect();
        let request = transactions::get_unconfirmed_transactions_decode_request(&field(
            2,
            ids.as_flattened(),
        ))
        .unwrap();
        assert_eq!(request.exclude, ids[..n.min(MAX_EXCLUSIONS + 1)], "{n}");
    }
    for length in [1, 31, 33, 63, 65] {
        let request =
            transactions::get_unconfirmed_transactions_decode_request(&field(2, &vec![7; length]))
                .unwrap();
        assert!(request.exclude.is_empty(), "{length} bytes");
        assert_eq!(
            request.request.exclude,
            Some(vec![7; length]),
            "the bytes as they came"
        );
    }
}

#[test]
fn list_edges_are_read_as_the_reference_reads_them() {
    let cases: [(&str, &str, &[&str]); 9] = [
        ("empty", "", &[]),
        ("4 bytes", "00000001", &[]),
        ("a length and 1 byte", "0000000199", &["99"]),
        ("a final empty item", "00000001aa00000000", &["aa"]),
        (
            "two final empty items",
            "00000001aa0000000000000000",
            &["aa", ""],
        ),
        (
            "a middle empty item",
            "00000001aa0000000000000001bb",
            &["aa", "", "bb"],
        ),
        ("3 trailing bytes", "00000001aaffffff", &["aa"]),
        ("5 trailing bytes", "00000001aa00000009ee", &["aa", "ee"]),
        ("an item that overruns", "ffffffff0102", &["0102"]),
    ];
    for (name, list, expected) in cases {
        let list = hex::decode(list).unwrap();
        let expected: Vec<Vec<u8>> = expected
            .iter()
            .map(|item| hex::decode(item).unwrap())
            .collect();
        let posted = transactions::post_transactions_decode_request(&field(1, &list)).unwrap();
        assert_eq!(posted.transactions, expected, "{name}");
        let listed = blocks::get_blocks_decode_response(&block_with_transactions(&list)).unwrap();
        assert_eq!(listed[0].transactions, expected, "{name} in a block");
    }
    // The block list itself: a final empty block is dropped, a middle one is a block of defaults.
    let one = uint(7, 1);
    let payload = [
        list::encode(&[one.clone(), vec![], one.clone()]),
        vec![0; 4],
    ]
    .concat();
    let listed = blocks::get_blocks_decode_response(&payload).unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[1].header.height, None);
    assert_eq!(
        listed[1].header.own_keys(),
        ["totalAmount", "totalFee", "reward", "transactions"]
    );
}

#[test]
fn amounts_are_read_as_bigint_reads_them() {
    let read = |text: &str| {
        let payload = list::encode(&[field(9, text.as_bytes())]);
        blocks::get_blocks_decode_response(&payload).map(|listed| listed[0].total_amount)
    };
    for (text, value) in [
        ("0x10", 16),
        ("0X1f", 31),
        ("0o17", 15),
        ("0b101", 5),
        (" 12\n", 12),
        ("", 0),
        ("\u{a0}\u{feff}7\u{2028}", 7),
        ("-0", 0),
        ("+5", 5),
        ("-5", -5),
        ("170141183460469231731687303715884105727", i128::MAX),
        ("-170141183460469231731687303715884105728", i128::MIN),
    ] {
        assert_eq!(read(text), Ok(Amount::new(value)), "{text:?}");
    }
    for text in [
        "1.5", "1e3", "0x", "-0x10", "+0x10", "1 2", "\u{85}7", "Infinity",
    ] {
        assert_eq!(read(text), Err(DecodeError::AmountSyntax), "{text:?}");
    }
    for text in [
        "170141183460469231731687303715884105728",
        "-170141183460469231731687303715884105729",
    ] {
        assert_eq!(read(text), Err(DecodeError::AmountTooWide), "{text:?}");
    }
    // Absent amounts are the empty text, which is 0; the first refusal in the list wins.
    let listed = blocks::get_blocks_decode_response(&list::encode(&[uint(7, 3)])).unwrap();
    assert_eq!(
        (listed[0].total_fee, listed[0].reward),
        (Amount::ZERO, Amount::ZERO)
    );
    assert_eq!(listed[0].header.total_fee.as_deref(), Some(""));
    let bad_then_malformed = list::encode(&[field(10, b"x"), vec![0x17]]);
    assert_eq!(
        blocks::get_blocks_decode_response(&bad_then_malformed),
        Err(DecodeError::AmountSyntax)
    );
    // An amount beyond 128 bits is refused only where the reference raises no error.
    let wide = "9".repeat(50);
    let wide_then_malformed = list::encode(&[field(9, wide.as_bytes()), vec![0x17]]);
    assert_eq!(
        blocks::get_blocks_decode_response(&wide_then_malformed),
        Err(DecodeError::InvalidWireType(7))
    );
    let wide_then_bad = list::encode(&[[field(9, wide.as_bytes()), field(10, b"x")].concat()]);
    assert_eq!(
        blocks::get_blocks_decode_response(&wide_then_bad),
        Err(DecodeError::AmountSyntax)
    );
    let malformed_then_bad = list::encode(&[vec![0x17], field(10, b"x")]);
    assert_eq!(
        blocks::get_blocks_decode_response(&malformed_then_bad),
        Err(DecodeError::InvalidWireType(7))
    );
}

#[test]
fn the_keys_the_codecs_set_keep_their_place_or_go_last() {
    let headers = [field(1, b"4.3.1"), uint(2, 4002)].concat();
    let request = blocks::post_block_decode_request(&field(2, &headers)).unwrap();
    assert_eq!(request.block.as_deref(), Some(&[][..]));
    assert_eq!(request.own_keys(), ["headers", "block"]);
    let request =
        blocks::post_block_decode_request(&[field(1, &[1]), field(2, &headers)].concat()).unwrap();
    assert_eq!(request.own_keys(), ["block", "headers"]);

    let request = transactions::get_unconfirmed_transactions_decode_request(&uint(1, 1)).unwrap();
    assert_eq!(request.request.own_keys(), ["countOnly", "exclude"]);
    let posted = transactions::post_transactions_decode_request(&field(2, &headers)).unwrap();
    assert_eq!(posted.message.own_keys(), ["headers", "transactions"]);
    let pool = transactions::get_unconfirmed_transactions_decode_response(
        &[field(3, &headers), uint(1, 9)].concat(),
    )
    .unwrap();
    assert_eq!(
        pool.message.own_keys(),
        ["headers", "poolSize", "transactions"]
    );

    let header = [
        field(11, b"5"),
        field(16, &[]),
        field(1, b"ab"),
        uint(7, 4),
        field(9, b"1"),
    ]
    .concat();
    let listed = blocks::get_blocks_decode_response(&list::encode(&[header])).unwrap();
    assert_eq!(
        listed[0].header.own_keys(),
        [
            "reward",
            "transactions",
            "id",
            "height",
            "totalAmount",
            "totalFee"
        ]
    );
}

#[test]
fn the_clients_requests_are_written_as_the_reference_writes_them() {
    let headers = Headers {
        version: Some("4.3.1".into()),
        port: Some(0),
        ..Headers::default()
    };
    // The forger asks for the pool with its headers only: the exclude list is written anyway.
    let request = UnconfirmedRequestInput {
        headers: Some(headers.clone()),
        ..Default::default()
    };
    assert_eq!(
        hex::encode(transactions::get_unconfirmed_transactions_encode_request(
            &request
        )),
        "12001a090a05342e332e311000"
    );
    let posted = PostTransactionsInput {
        transactions: vec![vec![0xaa], vec![]],
        headers: None,
    };
    assert_eq!(
        hex::encode(transactions::post_transactions_encode_request(&posted)),
        "0a0900000001aa00000000"
    );
    let pool = WithTransactions {
        message: GetUnconfirmedTransactionsResponse {
            pool_size: Some(0),
            ..Default::default()
        },
        transactions: vec![],
    };
    assert_eq!(
        hex::encode(transactions::get_unconfirmed_transactions_encode_response(
            &pool
        )),
        "08001200"
    );
    let block = PostBlockRequest {
        block: Some(vec![1, 2]),
        headers: Some(headers),
        ..Default::default()
    };
    assert_eq!(
        hex::encode(blocks::post_block_encode_request(&block)),
        "0a02010212090a05342e332e311000"
    );
    let reply = PostBlockResponse {
        status: Some(true),
        height: Some(12),
        ..Default::default()
    };
    assert_eq!(
        hex::encode(blocks::post_block_encode_response(&reply)),
        "0801100c"
    );
}

#[test]
fn a_headers_only_block_writes_an_empty_transaction_list() {
    let block = ListedBlock {
        header: BlockHeader {
            height: Some(3),
            total_amount: Some("ignored".into()),
            ..Default::default()
        },
        total_amount: Amount::from(u64::MAX),
        total_fee: Amount::new(9),
        reward: Amount::ZERO,
        transactions: vec![],
    };
    let bytes = blocks::get_blocks_encode_response(std::slice::from_ref(&block));
    assert_eq!(
        hex::encode(&bytes),
        "0000002138034a1431383434363734343037333730393535313631355201395a0130820100"
    );
    let decoded = blocks::get_blocks_decode_response(&bytes).unwrap();
    assert_eq!(
        (
            decoded[0].total_amount,
            decoded[0].total_fee,
            decoded[0].reward
        ),
        (block.total_amount, block.total_fee, block.reward)
    );
    assert!(blocks::get_blocks_encode_response(&[]).is_empty());
}

// The wire vectors.

const ROUTES: [&str; 4] = [
    "getBlocks",
    "postBlock",
    "getUnconfirmedTransactions",
    "postTransactions",
];

#[test]
fn the_codec_records_of_the_wire_vectors_match_the_reference() {
    let mut checked = 0;
    for record in common::records().iter().map(common::selected_as_decode) {
        let record = &record;
        let input = &record["input"];
        let (Some(route), Some(direction)) = (input["route"].as_str(), input["direction"].as_str())
        else {
            continue;
        };
        if !ROUTES.contains(&route) {
            continue;
        }
        let label = format!("{route} {direction}: {}", record["name"]);
        match record["op"].as_str() {
            Some("wire.encode") => {
                let bytes = encode(route, direction, &input["value"]);
                assert_eq!(
                    json!({"hex": hex::encode(bytes)}),
                    record["output"],
                    "{label}"
                );
            }
            Some("wire.decode") => {
                let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
                let overflow = record["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("Maximum call stack size exceeded"))
                    || record["output"]["notes"]["stackOverflow"] == true;
                match (decode(route, direction, &bytes), record.get("output")) {
                    // Groups nested beyond the reference's stack: Heartwood reads them (documented).
                    (Ok(_), _) if overflow => {}
                    (Ok(value), Some(output)) => {
                        assert!(output.get("notes").is_none(), "{label}: notes");
                        let expected = &output["value"];
                        assert_eq!(value.to_string(), expected.to_string(), "{label}");
                    }
                    (Err(DecodeError::AmountTooWide), Some(output)) => {
                        assert_eq!(
                            output["notes"]["wideAmount"], true,
                            "{label}: the reference reads a wide amount"
                        );
                    }
                    (Err(error), None) => {
                        assert_eq!(
                            error.js_class(),
                            record["error"]["class"].as_str(),
                            "{label}"
                        );
                    }
                    (ours, _) => panic!("{label}: Heartwood {ours:?}, the reference {record}"),
                }
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(
        checked > 0,
        "the wire vectors have no records of the block and transaction routes"
    );
}

/// `wire.encode`: the payload of `value`, read from the oracle's JSON.
fn encode(route: &str, direction: &str, value: &Value) -> Vec<u8> {
    let headers = || {
        value.get("headers").map(|h| Headers {
            version: h.get("version").map(|v| v.as_str().unwrap().to_owned()),
            port: h.get("port").map(|p| p.as_u64().unwrap() as u32),
            ..Headers::default()
        })
    };
    let hexes = |v: &Value| -> Vec<Vec<u8>> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|t| hex::decode(t.as_str().unwrap()).unwrap())
            .collect()
    };
    let u32_of = |v: &Value, key: &str| v.get(key).map(|n| n.as_u64().unwrap() as u32);
    let text_of = |v: &Value, key: &str| v.get(key).map(|s| s.as_str().unwrap().to_owned());
    match (route, direction) {
        ("getBlocks", "request") => blocks::get_blocks_encode_request(
            &heartwood_p2p_wire::messages::blocks::GetBlocksRequest {
                last_block_height: u32_of(value, "lastBlockHeight"),
                block_limit: u32_of(value, "blockLimit"),
                headers_only: value.get("headersOnly").map(|b| b.as_bool().unwrap()),
                serialised: value.get("serialised").map(|b| b.as_bool().unwrap()),
                headers: headers(),
                ..Default::default()
            },
        ),
        ("getBlocks", "response") => {
            let listed: Vec<ListedBlock> = value
                .as_array()
                .unwrap()
                .iter()
                .map(|b| ListedBlock {
                    header: BlockHeader {
                        id: text_of(b, "id"),
                        version: u32_of(b, "version"),
                        timestamp: u32_of(b, "timestamp"),
                        previous_block: text_of(b, "previousBlock"),
                        height: u32_of(b, "height"),
                        number_of_transactions: u32_of(b, "numberOfTransactions"),
                        payload_length: u32_of(b, "payloadLength"),
                        payload_hash: text_of(b, "payloadHash"),
                        generator_public_key: text_of(b, "generatorPublicKey"),
                        block_signature: text_of(b, "blockSignature"),
                        ..Default::default()
                    },
                    total_amount: Amount::parse(b["totalAmount"].as_str().unwrap()).unwrap(),
                    total_fee: Amount::parse(b["totalFee"].as_str().unwrap()).unwrap(),
                    reward: Amount::parse(b["reward"].as_str().unwrap()).unwrap(),
                    transactions: b.get("transactions").map(hexes).unwrap_or_default(),
                })
                .collect();
            blocks::get_blocks_encode_response(&listed)
        }
        ("postBlock", "request") => blocks::post_block_encode_request(&PostBlockRequest {
            block: value
                .get("block")
                .map(|b| hex::decode(b.as_str().unwrap()).unwrap()),
            headers: headers(),
            ..Default::default()
        }),
        ("postBlock", "response") => blocks::post_block_encode_response(&PostBlockResponse {
            status: value.get("status").map(|b| b.as_bool().unwrap()),
            height: u32_of(value, "height"),
            ..Default::default()
        }),
        ("getUnconfirmedTransactions", "request") => {
            transactions::get_unconfirmed_transactions_encode_request(&UnconfirmedRequestInput {
                count_only: value.get("countOnly").map(|b| b.as_bool().unwrap()),
                headers: headers(),
                exclude: value
                    .get("exclude")
                    .map(|ids| {
                        hexes(ids)
                            .into_iter()
                            .map(|id| id.try_into().unwrap())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        }
        ("getUnconfirmedTransactions", "response") => {
            transactions::get_unconfirmed_transactions_encode_response(&WithTransactions {
                message: GetUnconfirmedTransactionsResponse {
                    pool_size: u32_of(value, "poolSize"),
                    headers: headers(),
                    ..Default::default()
                },
                transactions: hexes(&value["transactions"]),
            })
        }
        ("postTransactions", "request") => {
            transactions::post_transactions_encode_request(&PostTransactionsInput {
                headers: headers(),
                transactions: hexes(&value["transactions"]),
            })
        }
        ("postTransactions", "response") => {
            let accept: Vec<String> = value
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect();
            transactions::post_transactions_encode_response(&accept)
        }
        other => panic!("no codec {other:?}"),
    }
}

/// `wire.decode`: the decoded value in the oracle's JSON, own keys in the reference's order.
fn decode(route: &str, direction: &str, bytes: &[u8]) -> Result<Value, DecodeError> {
    Ok(match (route, direction) {
        ("getBlocks", "request") => {
            let r = blocks::get_blocks_decode_request(bytes)?;
            object(&r, |key| match key {
                "lastBlockHeight" => json!(r.last_block_height),
                "blockLimit" => json!(r.block_limit),
                "headersOnly" => json!(r.headers_only),
                "serialised" => json!(r.serialised),
                "headers" => headers_value(r.headers.as_ref()),
                other => panic!("key {other}"),
            })
        }
        ("getBlocks", "response") => {
            let listed = blocks::get_blocks_decode_response(bytes)?;
            Value::Array(
                listed
                    .iter()
                    .map(|b| {
                        let h = &b.header;
                        object(h, |key| match key {
                            "id" => json!(h.id),
                            "version" => json!(h.version),
                            "timestamp" => json!(h.timestamp),
                            "previousBlock" => json!(h.previous_block),
                            "height" => json!(h.height),
                            "numberOfTransactions" => json!(h.number_of_transactions),
                            "totalAmount" => json!(b.total_amount.to_string()),
                            "totalFee" => json!(b.total_fee.to_string()),
                            "reward" => json!(b.reward.to_string()),
                            "payloadLength" => json!(h.payload_length),
                            "payloadHash" => json!(h.payload_hash),
                            "generatorPublicKey" => json!(h.generator_public_key),
                            "blockSignature" => json!(h.block_signature),
                            "transactions" => hexes(&b.transactions),
                            other => panic!("key {other}"),
                        })
                    })
                    .collect(),
            )
        }
        ("postBlock", "request") => {
            let r = blocks::post_block_decode_request(bytes)?;
            object(&r, |key| match key {
                "block" => json!(hex::encode(r.block.as_deref().unwrap())),
                "headers" => headers_value(r.headers.as_ref()),
                other => panic!("key {other}"),
            })
        }
        ("postBlock", "response") => {
            let r = blocks::post_block_decode_response(bytes)?;
            object(&r, |key| match key {
                "status" => json!(r.status),
                "height" => json!(r.height),
                other => panic!("key {other}"),
            })
        }
        ("getUnconfirmedTransactions", "request") => {
            let d = transactions::get_unconfirmed_transactions_decode_request(bytes)?;
            object(&d.request, |key| match key {
                "countOnly" => json!(d.request.count_only),
                "exclude" => hexes(&d.exclude),
                "headers" => headers_value(d.request.headers.as_ref()),
                other => panic!("key {other}"),
            })
        }
        ("getUnconfirmedTransactions", "response") => {
            let d = transactions::get_unconfirmed_transactions_decode_response(bytes)?;
            object(&d.message, |key| match key {
                "poolSize" => json!(d.message.pool_size),
                "transactions" => hexes(&d.transactions),
                "headers" => headers_value(d.message.headers.as_ref()),
                other => panic!("key {other}"),
            })
        }
        ("postTransactions", "request") => {
            let d = transactions::post_transactions_decode_request(bytes)?;
            object(&d.message, |key| match key {
                "transactions" => hexes(&d.transactions),
                "headers" => headers_value(d.message.headers.as_ref()),
                other => panic!("key {other}"),
            })
        }
        ("postTransactions", "response") => {
            json!(transactions::post_transactions_decode_response(bytes)?)
        }
        other => panic!("no codec {other:?}"),
    })
}

fn object<M: Proto>(message: &M, mut field: impl FnMut(&str) -> Value) -> Value {
    Value::Object(
        message
            .own_keys()
            .into_iter()
            .map(|key| (key.to_owned(), field(key)))
            .collect::<Map<_, _>>(),
    )
}

fn headers_value(headers: Option<&Headers>) -> Value {
    let headers = headers.unwrap();
    object(headers, |key| match key {
        "version" => json!(headers.version),
        "port" => json!(headers.port),
        other => panic!("key {other}"),
    })
}

fn hexes<T: AsRef<[u8]>>(items: &[T]) -> Value {
    items.iter().map(|item| json!(hex::encode(item))).collect()
}
