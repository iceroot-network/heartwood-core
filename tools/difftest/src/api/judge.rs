//! Judging the recorded answers of the route corpus.
//!
//! For each request: equal status codes; `connection: close` on both answers; `X-Block-Height`
//! on both, and equal (both nodes are at the paused tip); every other header equal, apart from
//! `date`, `content-length`, `transfer-encoding` and `keep-alive`; and bodies equal after the
//! projections:
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
//! test shows the documented behaviour.

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
    headers(lh, rh, &mut differences);
    if ls == rs {
        differences.extend(bodies(route, *rs, lb, rb));
    }
    if differences.is_empty() {
        return Outcome::Pass;
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
    for name in names {
        if !UNCOMPARED.contains(&name.as_str()) && left.get(name) != right.get(name) {
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
            // Ties may fall on either side of a page's end, so only the sort keys are compared.
            let keys = |body: &Value| {
                let projected = project(route, 200, body);
                projected["data"].as_array().map(|items| {
                    items
                        .iter()
                        .map(|t| t[property].clone())
                        .collect::<Vec<_>>()
                })
            };
            (left_status == right_status
                && left["meta"] == right["meta"]
                && keys(left) == keys(right))
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
    use super::*;

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
