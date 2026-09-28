//! Judging the recorded answers of the route corpus.
//!
//! For each request: equal status codes; `connection: close` on both answers; `X-Block-Height`
//! on both, and equal (both nodes are at the paused tip); every other header equal, apart from
//! `date`, `content-length`, `transfer-encoding` and `keep-alive` (and, on the peer list, the
//! `accept-encoding` of `vary`, which follows the length of a body that holds each node's own
//! peers); and bodies equal after the projections:
//!
//! - `confirmations` is left out everywhere, `/node/status`'s `timestamp` and `blocksCount`,
//!   `/node/syncing`'s `blocks`, `/node/configuration`'s `core.version` and a wallet's
//!   `attributes.delegate.version`: each depends on the time, the node's build or its own view of
//!   the network, not on the chain; a wallet's `attributes` are compared without their key order,
//!   which follows the node's history (a state rebuilt at a restart orders them otherwise);
//! - an error's `message` is not compared for 422, whose text is Joi's (the status is);
//! - the peer routes compare the peers both nodes list, by address, without `latency` and
//!   `height` (each node's own measurement and last report), and not their order or count;
//! - the pool list compares its `meta`, and its transactions as a set when both pages hold the
//!   whole pool (the order of equal priorities is the order of arrival).
//!
//! A case marked with a documented difference (`heightAboveTip`, `storeFailure`, `peersIpFilter`,
//! `tieOrder`, `decodeReason`) passes as documented when the answers differ and the node under
//! test shows the documented behaviour. A mark excuses the status and the body only: the headers
//! are judged as for any case, apart from those that follow the status ([`STATUS_HEADERS`]) when
//! the statuses differ. For `tieOrder`, both pages must hold the same sequence of the sorted
//! column's values, as the list's items show them; a transaction both pages hold must be the same
//! on both, and one that only one page holds must have the sorted value of the page's first or
//! last item (a tie that the page's start or end cuts).

use std::collections::BTreeMap;

use heartwood_api::RouteId;
use heartwood_api::schemas::parse_query;
use serde_json::Value;

use super::http::Reply;
use super::record::Recorded;
use crate::compare::{self, Difference};
use crate::report::Outcome;
use crate::subject::{ApiDifference, Documented};

/// Headers that are not compared: the time, the body's length and framing, and the two that are
/// checked on their own.
const UNCOMPARED: [&str; 6] = [
    "date",
    "content-length",
    "transfer-encoding",
    "keep-alive",
    "connection",
    "x-block-height",
];

/// Headers that follow the status: hapi offers byte ranges only in a successful answer.
const STATUS_HEADERS: [&str; 1] = ["accept-ranges"];

/// The served route of `method` and `path`: a literal segment wins over a parameter, as in the
/// reference's router.
pub fn route_of(method: &str, path: &str) -> Option<RouteId> {
    let path = path.strip_prefix("/api")?;
    let path = path
        .strip_suffix('/')
        .filter(|p| !p.is_empty())
        .unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    RouteId::ALL
        .into_iter()
        .filter(|route| format!("{:?}", route.method()).eq_ignore_ascii_case(method))
        .filter_map(|route| {
            let pattern: Vec<&str> = route.path().split('/').collect();
            let fits = pattern.len() == segments.len()
                && pattern
                    .iter()
                    .zip(&segments)
                    .all(|(p, s)| p == s || (p.starts_with('{') && !s.is_empty()));
            let literal = pattern.iter().filter(|p| !p.starts_with('{')).count();
            fits.then_some((literal, route))
        })
        .max_by_key(|(literal, _)| *literal)
        .map(|(_, route)| route)
}

/// The outcome of one recorded request.
pub fn outcome(line: &Recorded) -> Outcome {
    if let Some(reason) = &line.skipped {
        return Outcome::Skipped(reason.clone());
    }
    let (Some(left), Some(right)) = (&line.left, &line.right) else {
        return Outcome::HarnessError("a request without both answers".into());
    };
    let (
        Reply::Answer {
            status: ls,
            headers: lh,
            body: lb,
        },
        Reply::Answer {
            status: rs,
            headers: rh,
            body: rb,
        },
    ) = (left, right)
    else {
        return Outcome::HarnessError(format!("no answer: left {left:?}, right {right:?}"));
    };
    let request = line.request.as_ref();
    let route = request.and_then(|r| route_of(&r.method, &r.path));
    let mut differences = Vec::new();
    if ls != rs {
        differences.push(value("status", Value::from(*rs), Value::from(*ls)));
    }
    let mut header_differences = Vec::new();
    headers(lh, rh, route, &mut header_differences);
    // A documented difference never excuses a header, apart from one that follows a status that
    // differs.
    let follows_status = |d: &Difference| {
        ls != rs && (STATUS_HEADERS.iter()).any(|name| d.path() == format!("headers.{name}"))
    };
    let headers_differ = !header_differences.iter().all(follows_status);
    differences.extend(header_differences);
    if ls == rs {
        differences.extend(bodies(route, *rs, lb, rb));
    }
    if differences.is_empty() {
        return Outcome::Pass;
    }
    if headers_differ {
        return Outcome::Mismatch(differences);
    }
    let query = request
        .map(|r| parse_query(&r.query))
        .unwrap_or(Value::Null);
    match documented(
        line.documented.as_deref(),
        (*ls, lb),
        (*rs, rb),
        &query,
        route,
    ) {
        Some(reason) => Outcome::Documented(reason.to_owned()),
        None => Outcome::Mismatch(differences),
    }
}

fn value(path: &str, expected: Value, actual: Value) -> Difference {
    Difference::Value {
        path: path.to_owned(),
        expected,
        actual,
    }
}

fn headers(
    left: &serde_json::Map<String, Value>,
    right: &serde_json::Map<String, Value>,
    route: Option<RouteId>,
    out: &mut Vec<Difference>,
) {
    for (side, headers) in [("left", left), ("right", right)] {
        let close = headers
            .get("connection")
            .and_then(Value::as_str)
            .is_some_and(|v| v.eq_ignore_ascii_case("close"));
        if !close {
            out.push(value(
                &format!("headers.connection ({side})"),
                Value::from("close"),
                headers.get("connection").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    let height = |h: &serde_json::Map<String, Value>| h.get("x-block-height").cloned();
    if height(left).is_none() || height(left) != height(right) {
        out.push(value(
            "headers.x-block-height",
            height(right).unwrap_or(Value::Null),
            height(left).unwrap_or(Value::Null),
        ));
    }
    let names: std::collections::BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    // The peer list's body holds each node's own peers, of which only those both list are
    // compared; its length decides whether the answer may be compressed (1,024 bytes or more), and
    // with it whether `vary` names `accept-encoding`.
    let own_length = matches!(route, Some(RouteId::PeersIndex));
    let header = |headers: &serde_json::Map<String, Value>, name: &str| match headers.get(name) {
        Some(Value::String(vary)) if own_length && name == "vary" => Some(Value::from(
            vary.split(',')
                .map(str::trim)
                .filter(|v| !v.eq_ignore_ascii_case("accept-encoding"))
                .collect::<Vec<_>>()
                .join(","),
        )),
        other => other.cloned(),
    };
    for name in names {
        if !UNCOMPARED.contains(&name.as_str()) && header(left, name) != header(right, name) {
            out.push(value(
                &format!("headers.{name}"),
                right.get(name).cloned().unwrap_or(Value::Null),
                left.get(name).cloned().unwrap_or(Value::Null),
            ));
        }
    }
}

/// The differences of two bodies of the same status, after the route's projection.
fn bodies(route: Option<RouteId>, status: u16, left: &Value, right: &Value) -> Vec<Difference> {
    match route {
        Some(RouteId::PeersIndex | RouteId::PeersShow) if status == 200 => {
            let (l, r) = (peers(left), peers(right));
            l.iter()
                .filter_map(|(ip, peer)| r.get(ip).map(|other| (ip, peer, other)))
                .flat_map(|(ip, peer, other)| {
                    compare::diff(other, peer)
                        .into_iter()
                        .map(move |d| match d {
                            Difference::Value {
                                path,
                                expected,
                                actual,
                            } => value(&format!("peer {ip}{path}"), expected, actual),
                            other => other,
                        })
                })
                .collect()
        }
        Some(RouteId::TransactionsUnconfirmed) if status == 200 => {
            let mut out = compare::diff(&right["meta"], &left["meta"]);
            let whole = |b: &Value| b["meta"]["count"] == b["meta"]["totalCount"];
            if whole(left) && whole(right) {
                out.extend(compare::diff(&by_id(right), &by_id(left)));
            }
            out
        }
        _ => compare::diff(
            &project(route, status, right),
            &project(route, status, left),
        ),
    }
}

/// A body without the fields that do not depend on the chain.
fn project(route: Option<RouteId>, status: u16, body: &Value) -> Value {
    let mut body = body.clone();
    if status == 422 {
        remove(&mut body, &["message"]);
    }
    if status >= 400 {
        return body;
    }
    if let Some(data) = body.get_mut("data") {
        strip(data, "confirmations");
    }
    match route {
        Some(RouteId::NodeStatus) => {
            remove(&mut body, &["data", "timestamp"]);
            remove(&mut body, &["data", "blocksCount"]);
        }
        Some(RouteId::NodeSyncing) => remove(&mut body, &["data", "blocks"]),
        Some(RouteId::NodeConfiguration) => remove(&mut body, &["data", "core", "version"]),
        Some(RouteId::WalletsShow) => {
            remove(&mut body, &["data", "attributes", "delegate", "version"]);
            if let Some(attributes) = body.get_mut("data").and_then(|d| d.get_mut("attributes")) {
                sort_keys(attributes);
            }
        }
        _ => {}
    }
    body
}

fn remove(value: &mut Value, path: &[&str]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut at = value;
    for key in parents {
        match at.get_mut(*key) {
            Some(next) => at = next,
            None => return,
        }
    }
    if let Some(map) = at.as_object_mut() {
        map.shift_remove(*last);
    }
}

/// `value` with the keys of every object in sorted order.
fn sort_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.values_mut().for_each(sort_keys);
            map.sort_keys();
        }
        Value::Array(items) => items.iter_mut().for_each(sort_keys),
        _ => {}
    }
}

fn strip(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.shift_remove(key);
            map.values_mut().for_each(|v| strip(v, key));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| strip(v, key)),
        _ => {}
    }
}

/// The peers of a peer route's answer, by address, without their latency and height.
fn peers(body: &Value) -> BTreeMap<String, Value> {
    let data = &body["data"];
    let list = match data {
        Value::Array(items) => items.clone(),
        Value::Object(_) => vec![data.clone()],
        _ => Vec::new(),
    };
    list.into_iter()
        .filter_map(|mut peer| {
            let ip = peer["ip"].as_str()?.to_owned();
            remove(&mut peer, &["latency"]);
            remove(&mut peer, &["height"]);
            Some((ip, peer))
        })
        .collect()
}

/// The transactions of a list, by id.
fn by_id(body: &Value) -> Value {
    let map: serde_json::Map<String, Value> = body["data"]
        .as_array()
        .map(|items| {
            let mut pairs: Vec<(String, Value)> = items
                .iter()
                .map(|t| (t["id"].as_str().unwrap_or("").to_owned(), t.clone()))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs.into_iter().collect()
        })
        .unwrap_or_default();
    Value::Object(map)
}

/// The value of the stored column that an order on `property` sorts by, as the transaction `item`
/// of a list shows it (the resource, or the raw data with `transform=false`); `None` when the item
/// does not show it.
///
/// - `senderId` and `recipientId` are the resource's `sender` and `recipient` (the raw data's own
///   names otherwise);
/// - `amount` is stored only when it is above zero, and never for a transfer, whose resource shows
///   the sum of its transfers instead: the key is `null` for those;
/// - `memo`, `recipientId`, `amount` and `timestamp` may be absent (`null`; a transaction of the
///   genesis block shows no timestamp); any other column must be shown, so `sequence` needs the
///   raw data (which in turn has no `timestamp`).
fn sort_key(item: &Value, property: &str) -> Option<Value> {
    let shown = |names: &[&str]| names.iter().find_map(|name| item.get(*name)).cloned();
    match property {
        "senderId" => shown(&["sender", "senderId"]),
        "recipientId" => Some(shown(&["recipient", "recipientId"]).unwrap_or(Value::Null)),
        "memo" | "timestamp" => Some(shown(&[property]).unwrap_or(Value::Null)),
        "amount" => {
            let transfer = item["typeGroup"] == 1 && item["type"] == 6;
            let amount = shown(&["amount"]).filter(|a| !transfer && a != "0" && a != 0);
            Some(amount.unwrap_or(Value::Null))
        }
        other => shown(&[other]),
    }
}

/// The documented difference's reason, when the node under test shows its behaviour.
fn documented(
    kind: Option<&str>,
    (left_status, left): (u16, &Value),
    (right_status, right): (u16, &Value),
    query: &Value,
    route: Option<RouteId>,
) -> Option<&'static str> {
    match kind? {
        "heightAboveTip" => (left_status == 404 && left["message"] == "Block not found")
            .then(|| ApiDifference::HeightAboveTip.reason()),
        "storeFailure" => (left_status == 500 && left["error"] == "Internal Server Error")
            .then(|| ApiDifference::StoreFailure.reason()),
        "peersIpFilter" => {
            let ip = query["ip"].as_str()?;
            let filtered = left["data"].as_array()?.iter().all(|peer| peer["ip"] == ip);
            (left_status == 200 && right_status == 200 && filtered)
                .then(|| ApiDifference::PeersIpFilter.reason())
        }
        "tieOrder" => {
            let order = query["orderBy"]
                .as_str()
                .or_else(|| query["orderBy"][0].as_str())?;
            let property = order.split(':').next()?;
            // Ties may fall in any order, and on either side of a page's end: the pages must
            // hold the same sort keys, the same transaction alike, and a transaction on one page
            // only must belong to a tie that the page's start or end cuts. An answer that does
            // not show the sorted column cannot show a tie.
            let items = |body: &Value| project(route, 200, body)["data"].as_array().cloned();
            let (left_items, right_items) = (items(left)?, items(right)?);
            let keys = |items: &[Value]| -> Option<Vec<Value>> {
                items.iter().map(|t| sort_key(t, property)).collect()
            };
            let (l, r) = (keys(&left_items)?, keys(&right_items)?);
            let by_id = |items: &[Value]| -> Option<BTreeMap<String, Value>> {
                let id = |t: &Value| Some((t["id"].as_str()?.to_owned(), t.clone()));
                items.iter().map(id).collect()
            };
            let (left_ids, right_ids) = (by_id(&left_items)?, by_id(&right_items)?);
            let cut = [l.first(), l.last()];
            let alike = |one: &BTreeMap<String, Value>, other: &BTreeMap<String, Value>| {
                one.iter().all(|(id, item)| match other.get(id) {
                    Some(same) => same == item,
                    None => cut.contains(&sort_key(item, property).as_ref()),
                })
            };
            let same = alike(&left_ids, &right_ids) && alike(&right_ids, &left_ids);
            (left_status == right_status && left["meta"] == right["meta"] && l == r && same)
                .then(|| ApiDifference::TieOrder.reason())
        }
        "decodeReason" => {
            let reasons = |body: &Value| {
                let mut body = body.clone();
                if let Some(errors) = body.get_mut("errors").and_then(Value::as_object_mut) {
                    for error in errors.values_mut() {
                        let decode = error["message"]
                            .as_str()
                            .is_some_and(|m| m.starts_with("Invalid transaction data"));
                        if decode {
                            remove(error, &["message"]);
                        }
                    }
                }
                body
            };
            (left_status == right_status && reasons(left) == reasons(right))
                .then(|| Documented::DecodeReason.reason())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::api::http::Request;

    fn answer(status: u16, body: Value) -> Reply {
        let headers = json!({"connection": "close", "x-block-height": "265"});
        Reply::Answer {
            status,
            headers: headers.as_object().cloned().unwrap_or_default(),
            body,
        }
    }

    /// A recorded list request of `/api/transactions` with `query`, marked `documented`.
    fn list(query: &str, documented: &str, left: Reply, right: Reply) -> Recorded {
        Recorded {
            file: "transactions.json".into(),
            name: "case".into(),
            repeat: None,
            request: Some(Request {
                method: "GET".into(),
                path: "/api/transactions".into(),
                query: query.into(),
                headers: Vec::new(),
                body: None,
            }),
            outside: false,
            documented: Some(documented.into()),
            skipped: None,
            left: Some(left),
            right: Some(right),
        }
    }

    fn page(items: Value) -> Value {
        json!({"meta": {"count": 2, "totalCount": 2}, "data": items})
    }

    fn is_tie(outcome: &Outcome) -> bool {
        matches!(outcome, Outcome::Documented(r) if r == ApiDifference::TieOrder.reason())
    }

    #[test]
    fn a_tie_is_judged_on_the_sorted_column() {
        // Two transactions of one sender in either order: a tie of `senderId` (the resource's
        // `sender`).
        let a = json!({"id": "a", "sender": "dA", "fee": "1", "typeGroup": 1, "type": 2});
        let b = json!({"id": "b", "sender": "dA", "fee": "2", "typeGroup": 1, "type": 2});
        let swapped = |q: &str| {
            outcome(&list(
                q,
                "tieOrder",
                answer(200, page(json!([a, b]))),
                answer(200, page(json!([b, a]))),
            ))
        };
        assert!(is_tie(&swapped("orderBy=senderId:asc")));
        // Their fees differ, so an order on the fee is no tie.
        assert!(swapped("orderBy=fee:asc").is_failure());
        // The resource does not show the sequence: no tie can be seen.
        assert!(swapped("orderBy=sequence:asc").is_failure());
        // A transaction of the genesis block shows no timestamp: its rows tie on it.
        assert!(is_tie(&swapped("orderBy=timestamp:asc")));
        let raw = |sequence: u32| json!({"id": sequence.to_string(), "senderId": "dA", "sequence": sequence});
        assert!(is_tie(&outcome(&list(
            "orderBy=sequence:asc&transform=false",
            "tieOrder",
            answer(
                200,
                page(json!([raw(0), json!({"id": "x", "sequence": 0})]))
            ),
            answer(
                200,
                page(json!([json!({"id": "x", "sequence": 0}), raw(0)]))
            ),
        ))));
    }

    /// `reply` with the header `name` set to `text`.
    fn with_header(reply: Reply, name: &str, text: &str) -> Reply {
        match reply {
            Reply::Answer {
                status,
                mut headers,
                body,
            } => {
                headers.insert(name.into(), json!(text));
                Reply::Answer {
                    status,
                    headers,
                    body,
                }
            }
            other => other,
        }
    }

    #[test]
    fn a_tie_excuses_the_order_of_the_same_transactions_only() {
        let tx = |id: &str, sender: &str| json!({"id": id, "sender": sender, "fee": "1"});
        let (a, b, c) = (tx("a", "dA"), tx("b", "dA"), tx("c", "dB"));
        let judged = |left: Value, right: Value| {
            let page = |items: Value| json!({"meta": {"count": 3, "totalCount": 9}, "data": items});
            let (left, right) = (answer(200, page(left)), answer(200, page(right)));
            outcome(&list("orderBy=senderId:asc", "tieOrder", left, right))
        };
        assert!(is_tie(&judged(json!([b, a, c]), json!([a, b, c]))));
        // A field other than the sorted column differs on a transaction both pages hold.
        let mut changed = b.clone();
        changed["fee"] = json!("999");
        assert!(judged(json!([changed, a, c]), json!([a, b, c])).is_failure());
        // Another member of a tie that the page's end or start cuts.
        let (d, e) = (tx("d", "dB"), tx("e", "dA"));
        assert!(is_tie(&judged(json!([b, a, d]), json!([a, b, c]))));
        assert!(is_tie(&judged(json!([e, a, c]), json!([a, b, c]))));
        // A transaction that only one page holds, inside the page: no tie explains it.
        let (f, g) = (tx("f", "dB"), tx("g", "dB"));
        let (h, k) = (tx("h", "dC"), tx("k", "dC"));
        assert!(judged(json!([a, f, h]), json!([a, g, k])).is_failure());
        // A header that differs.
        let page = page(json!([b, a]));
        let left = with_header(answer(200, page.clone()), "cache-control", "no-store");
        let right = answer(200, json!({"meta": page["meta"], "data": [a, b]}));
        let right = with_header(right, "cache-control", "no-cache");
        assert!(outcome(&list("orderBy=senderId:asc", "tieOrder", left, right)).is_failure());
    }

    #[test]
    fn a_mark_excuses_only_the_headers_that_follow_a_status_that_differs() {
        let found = json!({"data": {"id": "x", "height": 300}});
        let missing =
            json!({"statusCode": 404, "error": "Not Found", "message": "Block not found"});
        let judged = |left: Reply, right: Reply| {
            let mut line = list("", "heightAboveTip", left, right);
            if let Some(request) = &mut line.request {
                request.path = "/api/blocks/300".into();
            }
            outcome(&line)
        };
        let ranged = |reply: Reply| with_header(reply, "accept-ranges", "bytes");
        let left = || answer(404, missing.clone());
        let right = || ranged(answer(200, found.clone()));
        assert!(matches!(
            judged(left(), right()),
            Outcome::Documented(r) if r == ApiDifference::HeightAboveTip.reason()
        ));
        for (name, text) in [("content-type", "text/html"), ("connection", "keep-alive")] {
            let left = with_header(left(), name, text);
            assert!(judged(left, right()).is_failure(), "{name}");
        }
        let left = with_header(left(), "x-block-height", "1");
        assert!(judged(left, right()).is_failure());
        // Equal statuses: the byte ranges must agree too.
        let equal = answer(404, missing.clone());
        assert!(judged(equal.clone(), ranged(equal)).is_failure());
    }

    #[test]
    fn an_amount_order_compares_the_stored_amounts() {
        // The store keeps no amount for a transfer (its resource shows the sum of its transfers)
        // and none for a zero amount: those rows tie, whatever their resources show.
        let transfer = json!({"id": "t", "typeGroup": 1, "type": 6, "amount": "10000"});
        let vote = json!({"id": "v", "typeGroup": 2, "type": 2});
        let burn = json!({"id": "b", "typeGroup": 2, "type": 0, "amount": "2000000"});
        let judged = |left: Value, right: Value| {
            outcome(&list(
                "orderBy=amount:asc",
                "tieOrder",
                answer(200, page(left)),
                answer(200, page(right)),
            ))
        };
        assert!(is_tie(&judged(
            json!([transfer, vote]),
            json!([vote, transfer])
        )));
        // A burn before the rows without an amount is not the order that puts them first.
        assert!(judged(json!([transfer, burn]), json!([burn, transfer])).is_failure());
        assert_eq!(
            sort_key(&json!({"amount": "0", "type": 0}), "amount"),
            Some(Value::Null)
        );
    }

    #[test]
    fn a_store_failure_is_a_500_against_an_empty_page() {
        let empty = json!({"meta": {"count": 0, "totalCount": 0}, "data": []});
        let failed = json!({"statusCode": 500, "error": "Internal Server Error", "message": "An internal server error occurred"});
        let judged = |left: Reply| {
            outcome(&list(
                "memo=%25%5C",
                "storeFailure",
                left,
                answer(200, empty.clone()),
            ))
        };
        assert!(matches!(
            judged(answer(500, failed)),
            Outcome::Documented(r) if r == ApiDifference::StoreFailure.reason()
        ));
        // Any other answer of the node under test is a difference.
        let other = json!({"meta": {"count": 1, "totalCount": 1}, "data": [{"id": "x"}]});
        assert!(judged(answer(200, other)).is_failure());
        // Equal answers pass whatever the mark.
        assert_eq!(judged(answer(200, empty.clone())), Outcome::Pass);
    }

    #[test]
    fn the_peer_list_s_vary_leaves_its_compression_out() {
        let peers = |left_vary: &str| {
            let with = |vary: &str| {
                let mut headers = serde_json::Map::new();
                headers.insert("connection".into(), json!("close"));
                headers.insert("x-block-height".into(), json!("265"));
                headers.insert("vary".into(), json!(vary));
                headers
            };
            let mut differences = Vec::new();
            headers(
                &with(left_vary),
                &with("origin"),
                Some(RouteId::PeersIndex),
                &mut differences,
            );
            let mut others = Vec::new();
            headers(
                &with(left_vary),
                &with("origin"),
                Some(RouteId::BlocksLast),
                &mut others,
            );
            (differences.len(), others.len())
        };
        // A longer list of the node's own peers is compressed: only there is it no difference.
        assert_eq!(peers("origin,accept-encoding"), (0, 1));
        assert_eq!(peers("origin"), (0, 0));
        assert_eq!(peers("accept-encoding"), (1, 1));
    }

    #[test]
    fn routes_match_literal_segments_first() {
        assert_eq!(
            route_of("GET", "/api/transactions/unconfirmed"),
            Some(RouteId::TransactionsUnconfirmed)
        );
        assert_eq!(
            route_of("GET", "/api/transactions/abc"),
            Some(RouteId::TransactionsShow)
        );
        assert_eq!(
            route_of("POST", "/api/transactions"),
            Some(RouteId::TransactionsStore)
        );
        assert_eq!(
            route_of("GET", "/api/blocks/last/"),
            Some(RouteId::BlocksLast)
        );
        assert_eq!(route_of("PUT", "/api/blocks/last"), None);
        assert_eq!(route_of("GET", "/api/nothing"), None);
    }
}
