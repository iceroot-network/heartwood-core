//! `p2p.validateRequest`: a request the client writes, as a reference server reads it (its payload
//! limit, its decoder, its version check and its request schema). The input is `{route, payload,
//! minimumVersions?, milestones?}`, the payload in the oracle's JSON with its `headers`; the output
//! is `{accepted: true, hex}` with the request's bytes, or `{accepted: false, status}` for a request
//! the server refuses.
//!
//! Heartwood's side is [`ClientRequest`]: a payload that it cannot express (no headers, an empty or
//! missing version, `countOnly`, no ids, a zero or missing height or limit, a key the client never
//! writes) is refused, as the reference's server refuses each of those shapes. A request it
//! builds is refused only when it is longer than the route's payload limit (413), or when the
//! server's version gate refuses its version (400): the milestone's floor, else the server's
//! `minimumVersions` option (the reference's default when the input names none).

use std::num::NonZeroU32;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{BlockId, TransactionId};
use heartwood_p2p::utils::is_valid_version::is_valid_version;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::requests::{ClientRequest, CommonBlockIds, Headers};
use serde_json::{Map, Value, json};

use crate::subject::{Case, Verdict};
use crate::subjects::{Env, Outcome, failed, params, text};

/// The reference's default `minimumVersions` option.
const DEFAULT_RANGES: [&str; 2] = ["^3.2", "^3.2.0-next.0"];

/// `p2p.validateRequest`.
pub(super) fn validate_request(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let refused = |status: u16| {
        Ok(Verdict::Output(
            json!({ "accepted": false, "status": status }),
        ))
    };
    let route = text(case.input, "route")?;
    let payload = case.input["payload"]
        .as_object()
        .ok_or_else(|| failed("input.payload is not an object"))?;
    let Some((request, headers)) = client_request(route, payload)? else {
        return refused(400);
    };
    let bytes = request.encode(&headers);
    let limit = request.route().max_bytes().map(usize::try_from);
    if limit.is_some_and(|limit| limit.is_ok_and(|limit| bytes.len() > limit)) {
        return refused(413);
    }
    let configured: Vec<String> = match case.input.get("minimumVersions") {
        None | Some(Value::Null) => DEFAULT_RANGES.map(str::to_owned).to_vec(),
        Some(Value::Array(ranges)) => (ranges.iter())
            .map(|range| range.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| failed("input.minimumVersions holds a value that is not text"))?,
        Some(_) => return Err(failed("input.minimumVersions is not a list")),
    };
    let milestones = env.milestones(case)?;
    let milestone = params(&milestones, case)?;
    if !is_valid_version(
        Some(headers.version()),
        milestone,
        &configured,
        case.network,
    ) {
        return refused(400);
    }
    Ok(Verdict::Output(
        json!({ "accepted": true, "hex": hex::encode(&bytes) }),
    ))
}

/// The client's request for `payload` on `route`, with its headers; `None` when the client cannot
/// write it.
fn client_request(
    route: &str,
    payload: &Map<String, Value>,
) -> Outcome<Option<(ClientRequest, Headers)>> {
    let route = (RouteId::ALL.into_iter().find(|r| r.name() == route))
        .ok_or_else(|| failed(format!("input.route: no route named {route}")))?;
    let keys: &[&str] = match route {
        RouteId::GetStatus | RouteId::GetPeers => &["headers"],
        RouteId::GetCommonBlocks => &["ids", "headers"],
        RouteId::GetBlocks => &[
            "lastBlockHeight",
            "blockLimit",
            "headersOnly",
            "serialised",
            "headers",
        ],
        RouteId::GetUnconfirmedTransactions => &["exclude", "headers"],
        RouteId::PostBlock => &["block", "headers"],
        RouteId::PostTransactions => &["transactions", "headers"],
        other => {
            return Err(failed(format!(
                "{other:?} is not a route the client requests"
            )));
        }
    };
    if payload.keys().any(|key| !keys.contains(&key.as_str())) {
        return Ok(None);
    }
    let Some(headers) = payload.get("headers").and_then(Value::as_object) else {
        return Ok(None);
    };
    if headers.keys().any(|key| key != "version" && key != "port") {
        return Ok(None);
    }
    let version = headers
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let port = (headers.get("port").and_then(Value::as_u64))
        .and_then(|port| u32::try_from(port).ok())
        .ok_or_else(|| failed("input.payload.headers.port is not a port"))?;
    let headers = match Headers::new(version, port) {
        // The reference's server refuses a request without a version, so the client must not be
        // able to write one: its refusal here is Heartwood's, not the server check's below.
        Ok(_) if version.is_empty() => {
            return Err(failed(
                "the client writes headers without a version, which the reference refuses",
            ));
        }
        Ok(headers) => headers,
        Err(_) => return Ok(None),
    };
    let hex_list = |key: &str| -> Outcome<Vec<Vec<u8>>> {
        (payload
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten())
        .map(|item| {
            (item.as_str().and_then(|text| hex::decode(text).ok()))
                .ok_or_else(|| failed(format!("input.payload.{key} holds a value that is not hex")))
        })
        .collect()
    };
    let height = |key: &str| {
        (payload.get(key).and_then(Value::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            .and_then(NonZeroU32::new)
    };
    let request = match route {
        RouteId::GetStatus => ClientRequest::GetStatus,
        RouteId::GetPeers => ClientRequest::GetPeers,
        RouteId::GetCommonBlocks => {
            let ids = (hex_list("ids")?.iter())
                .map(|id| BlockId::from_hex(&hex::encode(id)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| failed(format!("input.payload.ids: {e}")))?;
            match (payload.contains_key("ids"), CommonBlockIds::new(ids)) {
                (true, Ok(ids)) => ClientRequest::GetCommonBlocks { ids },
                _ => return Ok(None),
            }
        }
        RouteId::GetBlocks => {
            let serialised = payload.get("serialised") == Some(&Value::Bool(true));
            let headers_only = match payload.get("headersOnly") {
                None => false,
                Some(Value::Bool(true)) => true,
                Some(_) => return Ok(None),
            };
            match (height("lastBlockHeight"), height("blockLimit")) {
                (Some(last_block_height), Some(block_limit)) if serialised => {
                    ClientRequest::GetBlocks {
                        last_block_height,
                        block_limit,
                        headers_only,
                    }
                }
                _ => return Ok(None),
            }
        }
        RouteId::GetUnconfirmedTransactions => {
            if !payload.contains_key("exclude") {
                return Ok(None);
            }
            let exclude = (hex_list("exclude")?.iter())
                .map(|id| TransactionId::from_hex(&hex::encode(id)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| failed(format!("input.payload.exclude: {e}")))?;
            ClientRequest::GetUnconfirmedTransactions { exclude }
        }
        RouteId::PostBlock => {
            let block = (payload.get("block").and_then(Value::as_str))
                .and_then(|block| hex::decode(block).ok())
                .ok_or_else(|| failed("input.payload.block is not hex"))?;
            ClientRequest::PostBlock { block }
        }
        _ => {
            if !payload.contains_key("transactions") {
                return Ok(None);
            }
            ClientRequest::PostTransactions {
                transactions: hex_list("transactions")?,
            }
        }
    };
    Ok(Some((request, headers)))
}
