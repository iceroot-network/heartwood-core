//! Vector class V24 (the client's requests and the replies of the six routes other than
//! getStatus), compared with `requests` and `replies`.
//!
//! - `wire.checkReply`: the route's decoder and reply check; the verdict, the part a refusal
//!   names, and what the client reads of an accepted reply (addresses, the common block, each
//!   block's generator key, the postBlock height, the transactions and the accepted ids). Where
//!   the reference's stack overflowed or an amount is beyond 128 bits, only Heartwood's side of the
//!   documented difference is checked.
//! - `p2p.validateRequest`: a request the reference's server accepts, or refuses for its version,
//!   is one `ClientRequest` writes byte for byte; every other refused shape is one it cannot
//!   express.
//!
//! The class is read from `tests/vectors`, or from the directory in `WIRE_VECTORS` (a relative
//! path is taken from the workspace root), such as a local oracle run in `target/vectors/local`.
//! The crate's directory must hold the class, where `make vectors-install` puts it, so a missing
//! class fails the test; a directory named in the environment may lack it, and the test then says
//! so and passes.

use std::num::NonZeroU32;
use std::path::Path;

use heartwood_crypto::{BlockId, TransactionId};
use heartwood_p2p_wire::codecs::{blocks, peer, transactions};
use heartwood_p2p_wire::errors::{DecodeError, ReplyRefused};
use heartwood_p2p_wire::replies::{self, ROUTES};
use heartwood_p2p_wire::requests::{BlockLimit, ClientRequest, CommonBlockIds, Headers};
use heartwood_p2p_wire::{RouteId, routes};
use serde_json::{Value, json};

const CLASS: &str = "V24-p2p-exchange";

/// The class's records without its meta record, or `None` when the directory `WIRE_VECTORS` names
/// lacks it. The crate's own directory must hold it.
fn records() -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("WIRE_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        // A directory named in the environment may lack the class; the crate's own must hold it.
        Err(_) if named.is_some() => return None,
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    Some(lines.collect())
}

fn part(refused: ReplyRefused) -> &'static str {
    match refused {
        ReplyRefused::Root => "root",
        ReplyRefused::Header => "header",
        ReplyRefused::Peer => "peer",
        ReplyRefused::Common => "common",
        ReplyRefused::PostBlock => "postBlock",
        ReplyRefused::Unconfirmed => "unconfirmed",
        other => panic!("a getStatus part from another route: {other:?}"),
    }
}

/// Heartwood's answer to a `wire.checkReply` record: `Err` for a decoder refusal, else the check's
/// verdict with what the client reads of the reply.
fn check_reply(input: &Value) -> Result<Value, DecodeError> {
    let mut bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
    let route = match input["route"].as_str() {
        Some(name) => ROUTES.into_iter().find(|r| r.name() == name).unwrap(),
        None => {
            let first = if bytes.is_empty() { 0 } else { bytes.remove(0) };
            ROUTES[usize::from(first) % ROUTES.len()]
        }
    };
    let max = input["maxTransactions"]
        .as_u64()
        .map_or(150, |n| usize::try_from(n).unwrap());
    let verdict = |checked: Result<Value, ReplyRefused>| match checked {
        Ok(value) => json!({ "valid": true, "value": value }),
        Err(refused) => json!({ "valid": false, "refused": part(refused) }),
    };
    Ok(verdict(match route {
        RouteId::GetPeers => {
            replies::check_peers(&peer::get_peers_decode_response(&bytes)?).map(|addresses| {
                addresses
                    .iter()
                    .map(|a| json!({ "ip": a.ip, "port": a.port }))
                    .collect()
            })
        }
        RouteId::GetCommonBlocks => {
            let reply = peer::get_common_blocks_decode_response(&bytes)?;
            replies::check_common_blocks(&reply)
                .map(|common| json!({ "common": common.map(|c| json!({ "height": c.height, "id": c.id })) }))
        }
        RouteId::GetBlocks => {
            let mut listed = blocks::get_blocks_decode_response(&bytes)?;
            replies::check_blocks(&mut listed).map(|()| {
                (listed.iter())
                    .map(|block| json!({ "generatorPublicKey": block.header.generator_public_key }))
                    .collect()
            })
        }
        RouteId::PostBlock => {
            let reply = blocks::post_block_decode_response(&bytes)?;
            replies::check_post_block(&reply)
                .map(|height| json!({ "status": reply.status.unwrap_or(false), "height": height }))
        }
        RouteId::GetUnconfirmedTransactions => {
            let reply = transactions::get_unconfirmed_transactions_decode_response(&bytes)?;
            replies::check_unconfirmed(&reply, max).map(|()| {
                let listed: Vec<String> = reply.transactions.iter().map(hex::encode).collect();
                json!({ "transactions": listed })
            })
        }
        other => {
            assert_eq!(other, RouteId::PostTransactions);
            let accept = transactions::post_transactions_decode_response(&bytes)?;
            replies::check_post_transactions(&accept).map(|()| json!(accept))
        }
    }))
}

/// What the client reads of the oracle's reply value, in the shape of [`check_reply`].
fn read(route: Option<&str>, value: &Value) -> Value {
    match (route, value) {
        (Some("getBlocks"), Value::Array(blocks)) => (blocks.iter())
            .map(|block| json!({ "generatorPublicKey": block["generatorPublicKey"] }))
            .collect(),
        (Some("getUnconfirmedTransactions"), value) => {
            json!({ "transactions": value["transactions"] })
        }
        _ => value.clone(),
    }
}

/// The request `ClientRequest` writes for the payload, or `None` when it cannot express it.
fn client_request(route: &str, payload: &Value) -> Option<(ClientRequest, Headers)> {
    let payload = payload.as_object().unwrap();
    let headers = payload.get("headers")?.as_object()?;
    let version = headers
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let headers = Headers::new(version, u32::try_from(headers["port"].as_u64()?).ok()?).ok()?;
    let hex_list = |key: &str| -> Option<Vec<Vec<u8>>> {
        let items = payload.get(key)?.as_array()?;
        Some(
            items
                .iter()
                .map(|item| hex::decode(item.as_str().unwrap()).unwrap())
                .collect(),
        )
    };
    let height = |key: &str| NonZeroU32::new(u32::try_from(payload.get(key)?.as_u64()?).ok()?);
    let limit = |key: &str| BlockLimit::new(u32::try_from(payload.get(key)?.as_u64()?).ok()?);
    let allowed = |keys: &[&str]| {
        payload
            .keys()
            .all(|key| key == "headers" || keys.contains(&key.as_str()))
    };
    let request = match route {
        "getStatus" if allowed(&[]) => ClientRequest::GetStatus,
        "getPeers" if allowed(&[]) => ClientRequest::GetPeers,
        "getCommonBlocks" if allowed(&["ids"]) => {
            let ids: Vec<BlockId> = (hex_list("ids")?.iter())
                .map(|id| BlockId::from_hex(&hex::encode(id)).unwrap())
                .collect();
            ClientRequest::GetCommonBlocks {
                ids: CommonBlockIds::new(ids).ok()?,
            }
        }
        "getBlocks" if allowed(&["lastBlockHeight", "blockLimit", "headersOnly", "serialised"]) => {
            let headers_only = payload.get("headersOnly") == Some(&json!(true));
            if payload.get("serialised") != Some(&json!(true))
                || (payload.contains_key("headersOnly") && !headers_only)
            {
                return None;
            }
            ClientRequest::GetBlocks {
                last_block_height: height("lastBlockHeight")?,
                block_limit: limit("blockLimit")?,
                headers_only,
            }
        }
        "getUnconfirmedTransactions" if allowed(&["exclude"]) => {
            let exclude: Vec<TransactionId> = (hex_list("exclude")?.iter())
                .map(|id| TransactionId::from_hex(&hex::encode(id)).unwrap())
                .collect();
            ClientRequest::GetUnconfirmedTransactions { exclude }
        }
        "postBlock" if allowed(&["block"]) => ClientRequest::PostBlock {
            block: hex::decode(payload.get("block")?.as_str()?).unwrap(),
        },
        "postTransactions" if allowed(&["transactions"]) => ClientRequest::PostTransactions {
            transactions: hex_list("transactions")?,
        },
        _ => return None,
    };
    Some((request, headers))
}

fn check(record: &Value) {
    let (input, name) = (&record["input"], &record["name"]);
    match record["op"].as_str().unwrap() {
        "wire.checkReply" => {
            let output = &record["output"];
            let shown = |note: &str| output["notes"][note] == json!(true);
            let overflowed = record["error"]["class"] == "RangeError"
                && (record["error"]["message"].as_str())
                    .is_some_and(|m| m.starts_with("Maximum call stack"));
            match (check_reply(input), &record["error"]) {
                (Err(DecodeError::AmountTooWide), _) => assert!(shown("wideAmount"), "{name}"),
                (Err(error), Value::Null) => {
                    panic!("{name}: the reference accepts, Heartwood refuses: {error:?}")
                }
                (Err(error), expected) => {
                    assert_eq!(error.js_class(), expected["class"].as_str(), "{name}")
                }
                (Ok(_), _) if overflowed => {}
                (Ok(actual), Value::Null) => {
                    let mut expected = output.clone();
                    if let Some(object) = expected.as_object_mut() {
                        object.remove("notes");
                    }
                    let route = input["route"].as_str().or_else(|| {
                        let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
                        Some(ROUTES[usize::from(bytes.first().copied().unwrap_or(0)) % 6].name())
                    });
                    if expected["valid"] == json!(true) {
                        expected["value"] = read(route, &expected["value"]);
                    }
                    assert_eq!(actual, expected, "{name}");
                }
                (Ok(_), expected) => {
                    panic!("{name}: the reference refuses, Heartwood accepts: {expected}")
                }
            }
        }
        "p2p.validateRequest" => {
            let output = &record["output"];
            let route = input["route"].as_str().unwrap();
            let built = client_request(route, &input["payload"]);
            let refused_for_version =
                output["notes"]["message"] == "Validation failed (invalid version)";
            match (
                output["accepted"].as_bool().unwrap() || refused_for_version,
                built,
            ) {
                (true, Some((request, headers))) => {
                    let hex = output.get("hex").unwrap_or(&output["notes"]["hex"]);
                    assert_eq!(json!(hex::encode(request.encode(&headers))), *hex, "{name}");
                    assert_eq!(request.route().name(), route, "{name}");
                    let limit = routes::RouteId::max_bytes(request.route()).unwrap();
                    assert!(
                        request.encode(&headers).len() <= usize::try_from(limit).unwrap(),
                        "{name}"
                    );
                }
                (true, None) => {
                    panic!("{name}: the client cannot write a request the reference accepts")
                }
                (false, None) => {}
                (false, Some(_)) => {
                    panic!("{name}: the client writes a request the reference refuses")
                }
            }
        }
        other => panic!("{name}: an operation outside the class: {other}"),
    }
}

#[test]
fn the_requests_and_replies_match_the_reference() {
    let Some(records) = records() else {
        eprintln!("{CLASS} is not in the directory WIRE_VECTORS names");
        return;
    };
    assert!(records.iter().any(|r| r["op"] == "wire.checkReply"));
    assert!(records.iter().any(|r| r["op"] == "p2p.validateRequest"));
    for record in &records {
        check(record);
    }
}

#[test]
fn a_getblocks_limit_is_1_to_400() {
    assert_eq!(BlockLimit::MAX, 400);
    for refused in [0, 401, 1_000, u32::MAX] {
        assert_eq!(BlockLimit::new(refused), None, "{refused}");
    }
    for held in [1, 2, 399, 400] {
        assert_eq!(BlockLimit::new(held).map(BlockLimit::get), Some(held));
    }
    // The limit is written as it is held, at both edges.
    for held in [1, 400] {
        let request = ClientRequest::GetBlocks {
            last_block_height: NonZeroU32::MIN,
            block_limit: BlockLimit::new(held).unwrap(),
            headers_only: false,
        };
        let bytes = request.encode(&Headers::new("4.3.1", 4000).unwrap());
        let written = blocks::get_blocks_decode_request(&bytes).unwrap();
        assert_eq!(written.block_limit, Some(held));
    }
}
