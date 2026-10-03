//! Judging the recorded answers of the route corpus.
//!
//! For each request: equal status codes; `connection: close` on both answers; `X-Block-Height`
//! on both, and equal (both nodes are at the paused tip); every other header equal, apart from
//! `date`, `content-length`, `transfer-encoding` and `keep-alive`, and the `accept-encoding` of
//! `vary`, which follows the length of the answer's body (an answer of 1,024 bytes or more may be
//! compressed): it is judged on each answer against its own length (on the peer list, whose body
//! holds each node's own peers, not at all); and bodies equal after the projections:
//!
//! - `confirmations` is left out everywhere, `/node/status`'s `timestamp` and `blocksCount`,
//!   `/node/syncing`'s `blocks`, `/node/configuration`'s `core.version`, `/api.json`'s
//!   `info.version` and a wallet's `attributes.delegate.version`: each depends on the time, the
//!   node's build or its own view of the network, not on the chain; a wallet's `attributes` are compared without their key order,
//!   which follows the node's history (a state rebuilt at a restart orders them otherwise); the
//!   routes that list wallets (`/wallets`, `/wallets/top`, a delegate's voters) project each
//!   wallet as `/wallets/{id}` does;
//! - an error's `message` is not compared for 422, whose text is Joi's (the status is);
//! - the peer routes compare the peers both nodes list, by address, without `latency` and
//!   `height` (each node's own measurement and last report), and not their order or count;
//! - the pool list compares its `meta`, and its transactions as a set when both pages hold the
//!   whole pool (the order of equal priorities is the order of arrival).
//!
//! A live run's check may leave free, inside its tie, the place of a wallet one node of the pair
//! alone re-indexed by its own refusals, or of every wallet when both nodes run the reference
//! ([`outcome_with_free_places`], [`listed_wallets`]): nothing else may differ.
//!
//! A case marked with a documented difference (`heightAboveTip`, `storeFailure`, `peersIpFilter`,
//! `tieOrder`, `decodeReason`, `roundOrder`, `estimatedCount`, `feeTypes`, `specKeptOnly`)
//! passes as documented when the answers differ and the node under test shows the documented
//! behaviour. A mark excuses the status and the body only: the headers
//! are judged as for any case, apart from those that follow the status (`accept-ranges`) when the
//! statuses differ. For `tieOrder`, both pages must hold the same sequence of the sorted
//! column's values, as the list's items show them; a transaction both pages hold must be the same
//! on both, and one that only one page holds must have the sorted value of the page's first or
//! last item (a tie that the page's start or end cuts).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use heartwood_api::RouteId;
use heartwood_api::resources::fee_statistics::REGISTERED;
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

/// The length from which hapi may compress an answer, and so names `accept-encoding` in `vary`.
const COMPRESSED_FROM: usize = 1024;

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
    let lengths = (length(lh, lb), length(rh, rb));
    headers(lh, rh, route, lengths, &mut header_differences);
    // A documented difference never excuses a header, apart from one that follows a status that
    // differs.
    let follows_status = |d: &Difference| {
        ls != rs && (STATUS_HEADERS.iter()).any(|name| d.path() == format!("headers.{name}"))
    };
    let headers_differ = !header_differences.iter().all(follows_status);
    differences.extend(header_differences);
    if ls == rs {
        match request.filter(|r| is_spec(&r.path)) {
            Some(_) => {
                let (left, right) = (spec_projection(lb), spec_projection(rb));
                differences.extend(compare::diff(&right, &left));
            }
            None => differences.extend(bodies(route, *rs, lb, rb)),
        }
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

/// Where a listing over the state's indexes holds its wallets, and the properties its order ends
/// with after any `orderBy`: the balance for the wallet listings, the rank and the username for
/// the delegates (as the reference sorts them), none for the search's wallets, which the
/// reference lists in its index's order. `None` for another route.
fn listing(route: Option<RouteId>) -> Option<(&'static str, &'static [&'static str])> {
    match route? {
        RouteId::WalletsIndex | RouteId::WalletsTop | RouteId::DelegatesVoters => {
            Some(("/data", &["balance"]))
        }
        RouteId::DelegatesIndex => Some(("/data", &["rank", "username"])),
        RouteId::BlockchainSearch => Some(("/data/wallets", &[])),
        _ => None,
    }
}

/// The addresses of the wallets both answers of a recorded listing over the state's indexes
/// list ([`outcome_with_free_places`]'s routes), on either page; none for another route.
pub fn listed_wallets(line: &Recorded) -> BTreeSet<String> {
    let route = (line.request.as_ref()).and_then(|r| route_of(&r.method, &r.path));
    let Some((at, _)) = listing(route) else {
        return BTreeSet::new();
    };
    [&line.left, &line.right]
        .into_iter()
        .filter_map(|reply| match reply {
            Some(Reply::Answer { body, .. }) => body.pointer(at).and_then(Value::as_array),
            _ => None,
        })
        .flatten()
        .filter_map(|wallet| wallet["address"].as_str().map(str::to_owned))
        .collect()
}

/// The outcome of one recorded request of a listing over the state's indexes (`/wallets`,
/// `/wallets/top`, a delegate's voters, `/delegates`, the wallets `/blockchain/search` finds)
/// with the places of the wallets `free` left free inside their ties, and the free wallets whose
/// places the two answers hold apart.
///
/// The listings follow the indexes' orders in their ties, and a node's own refusals re-index
/// their parties in its state: a wallet whose transactions one node of the pair alone refused
/// may stand elsewhere in its tie on that node, with nothing else changed. The answers are
/// judged as [`outcome`] judges them after the control's page is put in the order of the node's
/// under test, which is allowed only when both pages hold the same wallets, the other wallets in
/// one order, and every pair of wallets the reordering swaps ties on the listing's order (the
/// `orderBy` properties, then the route's own: the balance for the wallet listings, the rank and
/// the username for the delegates; the search has no order, so its wallets tie). Any other
/// difference is judged as without it: a wrong tie order of any other wallet, a free wallet moved
/// across another value of the order, or a difference of a value still fails.
pub fn outcome_with_free_places(
    line: &Recorded,
    free: &BTreeSet<String>,
) -> (Outcome, Vec<String>) {
    let plain = outcome(line);
    if !plain.is_failure() || free.is_empty() {
        return (plain, Vec::new());
    }
    let request = line.request.as_ref();
    let route = request.and_then(|r| route_of(&r.method, &r.path));
    let Some((at, tail)) = listing(route) else {
        return (plain, Vec::new());
    };
    let (
        Some(Reply::Answer {
            status: 200,
            body: left,
            ..
        }),
        Some(Reply::Answer {
            status: 200,
            headers,
            body: right,
        }),
    ) = (&line.left, &line.right)
    else {
        return (plain, Vec::new());
    };
    let query = request
        .map(|r| parse_query(&r.query))
        .unwrap_or(Value::Null);
    let mut order = order_properties(&query);
    order.extend(tail.iter().map(|p| (*p).to_owned()));
    let ours = left.pointer(at).and_then(Value::as_array);
    let theirs = right.pointer(at).and_then(Value::as_array);
    let (Some(ours), Some(theirs)) = (ours, theirs) else {
        return (plain, Vec::new());
    };
    let Some((reordered, moved)) = free_reorder(ours, theirs, free, &order) else {
        return (plain, Vec::new());
    };
    let mut body = right.clone();
    let Some(list) = body.pointer_mut(at) else {
        return (plain, Vec::new());
    };
    *list = Value::Array(reordered);
    let mut freed = line.clone();
    freed.right = Some(Reply::Answer {
        status: 200,
        headers: headers.clone(),
        body,
    });
    match outcome(&freed) {
        Outcome::Pass => (Outcome::Pass, moved),
        _ => (plain, Vec::new()),
    }
}

/// The properties a query's `orderBy` sorts by, in order (`property:direction`, comma-separated
/// or repeated).
fn order_properties(query: &Value) -> Vec<String> {
    let entries: Vec<&str> = match &query["orderBy"] {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    entries
        .iter()
        .flat_map(|entry| entry.split(','))
        .filter_map(|one| one.split(':').next())
        .filter(|property| !property.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The value of an item at the dotted `path` (`attributes.delegate.producedBlocks`), `null` when
/// it has none.
fn at_path<'v>(item: &'v Value, path: &str) -> &'v Value {
    path.split('.').fold(item, |at, key| &at[key])
}

/// The control's page `theirs` in the order of `ours` and the free wallets it moves, when the two
/// pages differ only by the places of wallets of `free` inside their ties on `order`.
fn free_reorder(
    ours: &[Value],
    theirs: &[Value],
    free: &BTreeSet<String>,
    order: &[String],
) -> Option<(Vec<Value>, Vec<String>)> {
    let address = |item: &Value| item["address"].as_str().map(str::to_owned);
    let ours: Vec<String> = ours.iter().map(address).collect::<Option<_>>()?;
    let theirs_at: BTreeMap<String, (usize, &Value)> = (theirs.iter().enumerate())
        .map(|(at, item)| Some((address(item)?, (at, item))))
        .collect::<Option<_>>()?;
    // The same wallets on both pages, each once, and the others in one order.
    let ours_once: BTreeSet<&String> = ours.iter().collect();
    if theirs_at.len() != theirs.len()
        || ours_once.len() != ours.len()
        || ours.len() != theirs.len()
        || ours.iter().any(|a| !theirs_at.contains_key(a))
    {
        return None;
    }
    let others = |list: &mut dyn Iterator<Item = String>| -> Vec<String> {
        list.filter(|a| !free.contains(a)).collect()
    };
    let theirs_order: Vec<String> = theirs.iter().filter_map(address).collect();
    if others(&mut ours.iter().cloned()) != others(&mut theirs_order.into_iter()) {
        return None;
    }
    // Every pair the reordering swaps ties on the order.
    let key = |a: &str| -> Vec<Value> {
        let item = theirs_at.get(a).map_or(&Value::Null, |(_, item)| *item);
        order.iter().map(|p| at_path(item, p).clone()).collect()
    };
    let mut moved = BTreeSet::new();
    for (i, a) in ours.iter().enumerate() {
        for b in &ours[i + 1..] {
            if theirs_at[a].0 > theirs_at[b].0 {
                if key(a) != key(b) {
                    return None;
                }
                moved.extend([a, b].into_iter().filter(|x| free.contains(*x)).cloned());
            }
        }
    }
    if moved.is_empty() {
        return None;
    }
    let reordered = ours.iter().map(|a| theirs_at[a].1.clone()).collect();
    Some((reordered, moved.into_iter().collect()))
}

fn value(path: &str, expected: Value, actual: Value) -> Difference {
    Difference::Value {
        path: path.to_owned(),
        expected,
        actual,
    }
}

/// The length of an answer's body: its `content-length`, else the length of its JSON text.
fn length(headers: &serde_json::Map<String, Value>, body: &Value) -> usize {
    let stated = headers.get("content-length").and_then(Value::as_str);
    match stated.and_then(|text| text.parse().ok()) {
        Some(length) => length,
        None => match body {
            Value::String(text) => text.len(),
            other => other.to_string().len(),
        },
    }
}

fn headers(
    left: &serde_json::Map<String, Value>,
    right: &serde_json::Map<String, Value>,
    route: Option<RouteId>,
    (left_length, right_length): (usize, usize),
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
    // An answer's length decides whether it may be compressed, and with it whether `vary` names
    // `accept-encoding`: that part of `vary` is left out where it follows the answer's own length.
    // The peer list's body holds each node's own peers, of which only those both list are
    // compared, so there it is always left out.
    let own_peers = matches!(route, Some(RouteId::PeersIndex));
    // `vary` without `accept-encoding`, and whether naming it follows the answer's length.
    let vary = |headers: &serde_json::Map<String, Value>, length: usize| {
        let vary = headers
            .get("vary")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let encoding = |v: &&str| v.eq_ignore_ascii_case("accept-encoding");
        let named = vary.split(',').map(str::trim).any(|v| encoding(&v));
        let kept: Vec<&str> = (vary.split(',').map(str::trim))
            .filter(|v| !encoding(v))
            .collect();
        (
            kept.join(","),
            own_peers || named == (length >= COMPRESSED_FROM),
        )
    };
    for name in names {
        let differs = left.get(name) != right.get(name)
            && (name != "vary" || {
                let (ours, theirs) = (vary(left, left_length), vary(right, right_length));
                ours.0 != theirs.0 || !ours.1 || !theirs.1
            });
        if !UNCOMPARED.contains(&name.as_str()) && differs {
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
        Some(RouteId::WalletsShow) => body.get_mut("data").into_iter().for_each(wallet),
        Some(RouteId::WalletsIndex | RouteId::WalletsTop | RouteId::DelegatesVoters) => {
            let listed = body.get_mut("data").and_then(Value::as_array_mut);
            listed.into_iter().flatten().for_each(wallet);
        }
        _ => {}
    }
    body
}

/// A wallet without its delegate's version, its attributes in key order.
fn wallet(wallet: &mut Value) {
    remove(wallet, &["attributes", "delegate", "version"]);
    if let Some(attributes) = wallet.get_mut("attributes") {
        sort_keys(attributes);
    }
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
/// - `amount` is stored for a burn only (type group 2, type 0), zero included, as the store's
///   order reads it: the key is `null` for every other type, a transfer included, whose resource
///   shows the sum of its transfers instead;
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
            let burn = item["typeGroup"] == 2 && item["type"] == 0;
            Some(shown(&["amount"]).filter(|_| burn).unwrap_or(Value::Null))
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
        "storeFailure" => {
            // The reference's repository catches the store's error: an empty page.
            let empty = right_status == 200
                && right["meta"]["totalCount"] == 0
                && right["data"].as_array().is_some_and(Vec::is_empty);
            (left_status == 500 && left["error"] == "Internal Server Error" && empty)
                .then(|| ApiDifference::StoreFailure.reason())
        }
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
        "roundOrder" => {
            // The same delegates on both pages, Heartwood's in the stored order.
            let (ours, theirs) = (left["data"].as_array()?, right["data"].as_array()?);
            let stored = ours.windows(2).all(|pair| match pair {
                [a, b] => round_order(a, b) != Ordering::Greater,
                _ => true,
            });
            let same = as_set(ours) == as_set(theirs);
            (left_status == 200 && right_status == 200 && same && stored)
                .then(|| ApiDifference::RoundOrder.reason())
        }
        "estimatedCount" => {
            // The same page; the counts, and what follows from them, apart (past the last page,
            // whether there is a previous one).
            let counted = |body: &Value| {
                let mut body = body.clone();
                for key in ["totalCount", "pageCount", "last", "next", "previous"] {
                    remove(&mut body, &["meta", key]);
                }
                body
            };
            let exact = left["meta"]["totalCount"].as_u64()?;
            let shown = u64::try_from(left["data"].as_array().map_or(0, Vec::len)).ok()?;
            let same = counted(left) == counted(right);
            (left_status == 200 && right_status == 200 && same && exact >= shown)
                .then(|| ApiDifference::EstimatedCount.reason())
        }
        "feeTypes" => {
            // Heartwood's types are kept types, each with the reference's values; the reference's
            // others hold zeros only; and without `days`, where the reference lists every
            // registered type, Heartwood lists every kept one.
            let (ours, theirs) = (left["data"].as_object()?, right["data"].as_object()?);
            let kept = ours.iter().all(|(group, types)| {
                let types = types.as_object().into_iter().flatten();
                types.into_iter().all(|(name, value)| {
                    &right["data"][group][name] == value && is_kept(group, name)
                })
            });
            let others = theirs.iter().all(|(group, types)| {
                let types = types.as_object().into_iter().flatten();
                types.into_iter().all(|(name, value)| {
                    ours.get(group).and_then(|g| g.get(name)).is_some() || zeros(value)
                })
            });
            let every = left["meta"].get("days").is_some()
                || REGISTERED.iter().all(|kind| {
                    let group = ours.get(&kind.type_group().to_string());
                    group.and_then(|g| g.get(kind.key())).is_some()
                });
            let meta = left["meta"] == right["meta"];
            (left_status == 200 && right_status == 200 && meta && kept && others && every)
                .then(|| ApiDifference::FeeTypes.reason())
        }
        "specKeptOnly" => {
            // Heartwood's paths are the reference's but the removed lock operations', each the
            // same; its type and group lists are the kept types', which the reference's hold; and
            // the rest of the document is the same, apart from the version ([`spec_projection`]).
            let (ours, theirs) = (left["paths"].as_object()?, right["paths"].as_object()?);
            let kept = ours
                .iter()
                .all(|(path, item)| theirs.get(path) == Some(item));
            let removed: Vec<&str> = (theirs.keys())
                .filter(|path| !ours.contains_key(*path))
                .map(String::as_str)
                .collect();
            let listed = |body: &Value, name: &str| -> Option<Vec<u64>> {
                let list = body["components"]["schemas"][name]["enum"].as_array()?;
                list.iter().map(Value::as_u64).collect()
            };
            let kept_list = |name: &str, kept: Vec<u64>| {
                let (Some(mut ours), Some(theirs)) = (listed(left, name), listed(right, name))
                else {
                    return false;
                };
                ours.sort_unstable();
                ours == kept && kept.iter().all(|value| theirs.contains(value))
            };
            let set = |values: &mut Vec<u64>| {
                values.sort_unstable();
                values.dedup();
                std::mem::take(values)
            };
            let types = set(&mut REGISTERED.map(|kind| u64::from(kind.type_id())).to_vec());
            let groups = set(&mut REGISTERED.map(|kind| u64::from(kind.type_group())).to_vec());
            let rest = |body: &Value| {
                let mut body = spec_projection(body);
                remove(&mut body, &["paths"]);
                for list in ["transactionTypes", "transactionTypeGroups"] {
                    remove(&mut body, &["components", "schemas", list, "enum"]);
                }
                body
            };
            (left_status == 200
                && right_status == 200
                && kept
                && removed == REMOVED_PATHS
                && kept_list("transactionTypes", types)
                && kept_list("transactionTypeGroups", groups)
                && rest(left) == rest(right))
            .then(|| ApiDifference::SpecKeptOnly.reason())
        }
        _ => None,
    }
}

/// The paths of the reference's document that Heartwood's leaves out, in the document's order: the
/// lock (HTLC) operations, whose transaction types are removed.
const REMOVED_PATHS: [&str; 3] = ["/locks", "/locks/{id}", "/locks/unlocked"];

/// Whether `name` in type group `group` is a type Heartwood carries.
fn is_kept(group: &str, name: &str) -> bool {
    REGISTERED
        .iter()
        .any(|kind| kind.type_group().to_string() == group && kind.key() == name)
}

/// The OpenAPI document without its `info.version`, which names the node's build, as
/// `/node/configuration`'s `core.version` does.
fn spec_projection(body: &Value) -> Value {
    let mut body = body.clone();
    remove(&mut body, &["info", "version"]);
    body
}

/// Whether a request of `path` asks for the OpenAPI document.
fn is_spec(path: &str) -> bool {
    path.strip_suffix('/').unwrap_or(path) == "/api.json"
}

/// The order of a round's stored rows: votes descending, then the public key.
fn round_order(a: &Value, b: &Value) -> Ordering {
    let votes = |item: &Value| {
        let text = item["votes"].as_str().unwrap_or_default().to_owned();
        (text.len(), text)
    };
    let key = |item: &Value| item["publicKey"].as_str().unwrap_or_default().to_owned();
    votes(b).cmp(&votes(a)).then_with(|| key(a).cmp(&key(b)))
}

/// The items of a list as a sorted list of their JSON texts.
fn as_set(items: &[Value]) -> Vec<String> {
    let mut texts: Vec<String> = items.iter().map(Value::to_string).collect();
    texts.sort();
    texts
}

/// Whether every statistic of a type's fees is zero.
fn zeros(value: &Value) -> bool {
    value.as_object().is_some_and(|stats| {
        stats
            .values()
            .all(|v| v == "0" || v.as_u64() == Some(0) || v.as_f64() == Some(0.0))
    })
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

    /// A recorded request of `path`, marked `documented`.
    fn case(path: &str, documented: &str, left: Reply, right: Reply) -> Recorded {
        let mut line = list("", documented, left, right);
        if let Some(request) = &mut line.request {
            request.path = path.into();
        }
        line
    }

    fn is(outcome: &Outcome, difference: ApiDifference) -> bool {
        matches!(outcome, Outcome::Documented(r) if r == difference.reason())
    }

    /// A recorded `/api/wallets` request with `query`, its pages the wallets `left` and `right`
    /// (each an address and a balance).
    fn wallets(query: &str, left: &[(&str, u64)], right: &[(&str, u64)]) -> Recorded {
        let items = |list: &[(&str, u64)]| -> Value {
            let items: Vec<Value> = (list.iter())
                .map(|(a, b)| json!({"address": a, "balance": b.to_string(), "nonce": "1"}))
                .collect();
            json!({"meta": {"count": items.len(), "totalCount": items.len()}, "data": items})
        };
        let mut line = list(
            query,
            "",
            answer(200, items(left)),
            answer(200, items(right)),
        );
        line.documented = None;
        if let Some(request) = &mut line.request {
            request.path = "/api/wallets".into();
        }
        line
    }

    fn free(addresses: &[&str]) -> BTreeSet<String> {
        addresses.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn a_wallet_one_node_re_indexed_is_free_inside_its_tie_only() {
        let ours = [("A", 100), ("G", 50), ("B", 50), ("C", 50), ("D", 10)];
        let theirs = [("A", 100), ("B", 50), ("C", 50), ("G", 50), ("D", 10)];
        let line = wallets("", &ours, &theirs);
        assert!(outcome(&line).is_failure());
        // The re-indexed wallet's place is free in its tie.
        let (judged, moved) = outcome_with_free_places(&line, &free(&["G"]));
        assert_eq!((judged, moved), (Outcome::Pass, vec!["G".to_owned()]));
        // Without the evidence, or with another wallet's, the tie order fails.
        assert!(outcome_with_free_places(&line, &free(&[])).0.is_failure());
        assert!(
            outcome_with_free_places(&line, &free(&["B"]))
                .0
                .is_failure()
        );
        // Another wallet out of place as well: still a failure.
        let swapped = [("A", 100), ("C", 50), ("B", 50), ("G", 50), ("D", 10)];
        let line = wallets("", &ours, &swapped);
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
        // The free wallet across another value of the order: no tie explains it.
        let across = [("A", 100), ("B", 50), ("C", 50), ("D", 10), ("G", 50)];
        let line = wallets("", &ours, &across);
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
        // A value that differs is a difference whatever the places.
        let richer = [("A", 100), ("B", 50), ("C", 50), ("G", 51), ("D", 10)];
        let line = wallets("", &ours, &richer);
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
        // Another wallet on the page in its place (a page's end): not a place inside a tie.
        let other = [("A", 100), ("B", 50), ("C", 50), ("E", 50), ("D", 10)];
        let line = wallets("", &ours, &other);
        assert!(
            outcome_with_free_places(&line, &free(&["G", "E"]))
                .0
                .is_failure()
        );
        // A wallet twice on one page and another missing: not the same wallets.
        let twice = [("A", 100), ("G", 50), ("G", 50), ("C", 50), ("D", 10)];
        let line = wallets("", &twice, &theirs);
        assert!(
            outcome_with_free_places(&line, &free(&["G", "B"]))
                .0
                .is_failure()
        );
        // The tie is the order's: on `orderBy` and then the balance.
        let by_nonce =
            |a: &str, n: &str, b: u64| json!({"address": a, "balance": b.to_string(), "nonce": n});
        let page =
            |items: Vec<Value>| json!({"meta": {"count": 3, "totalCount": 3}, "data": items});
        let line = list(
            "orderBy=nonce:desc",
            "",
            answer(
                200,
                page(vec![
                    by_nonce("G", "2", 50),
                    by_nonce("B", "1", 50),
                    by_nonce("C", "1", 50),
                ]),
            ),
            answer(
                200,
                page(vec![
                    by_nonce("B", "1", 50),
                    by_nonce("G", "2", 50),
                    by_nonce("C", "1", 50),
                ]),
            ),
        );
        let mut line = line;
        line.documented = None;
        if let Some(request) = &mut line.request {
            request.path = "/api/wallets".into();
        }
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
        // The delegates' listing is ordered by rank, then username: a free delegate placed
        // against its rank is no tie.
        let delegate = |name: &str, rank: u64| json!({"username": name, "address": name, "rank": rank, "votes": "0"});
        let delegates = |items: Vec<Value>| {
            let mut line = list(
                "",
                "",
                answer(
                    200,
                    json!({"meta": {"count": 3, "totalCount": 3}, "data": [
                        delegate("B", 1), delegate("C", 2), delegate("G", 3)]}),
                ),
                answer(
                    200,
                    json!({"meta": {"count": 3, "totalCount": 3}, "data": items}),
                ),
            );
            line.documented = None;
            if let Some(request) = &mut line.request {
                request.path = "/api/delegates".into();
            }
            line
        };
        let line = delegates(vec![delegate("G", 3), delegate("B", 1), delegate("C", 2)]);
        assert!(outcome(&line).is_failure());
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
        // A route that is not a listing over the state is judged as it is.
        let mut blocks = wallets("", &ours, &theirs);
        if let Some(request) = &mut blocks.request {
            request.path = "/api/blocks".into();
        }
        assert!(
            outcome_with_free_places(&blocks, &free(&["G"]))
                .0
                .is_failure()
        );
    }

    /// A recorded `/api/blockchain/search/{term}` request, its answers' wallets `left` and
    /// `right` (each an address and a delegate's username).
    fn search(left: &[(&str, &str)], right: &[(&str, &str)]) -> Recorded {
        let found = |list: &[(&str, &str)]| -> Value {
            let wallets: Vec<Value> = (list.iter())
                .map(|(a, u)| json!({"address": a, "delegate": {"username": u, "rank": 1}}))
                .collect();
            json!({"data": {"blocks": [], "transactions": [], "wallets": wallets}})
        };
        let mut line = list("", "", answer(200, found(left)), answer(200, found(right)));
        line.documented = None;
        if let Some(request) = &mut line.request {
            request.path = "/api/blockchain/search/genesis_".into();
        }
        line
    }

    #[test]
    fn a_searched_wallet_one_node_re_indexed_is_free_and_nothing_else_is() {
        // The reference answers in its index's order, which its own refusals moved: the search
        // has no order of its own, so the moved wallet may stand anywhere in the list.
        let ours = [("G", "gina"), ("B", "bert"), ("C", "cara"), ("D", "dave")];
        let theirs = [("B", "bert"), ("C", "cara"), ("D", "dave"), ("G", "gina")];
        let line = search(&ours, &theirs);
        assert!(outcome(&line).is_failure());
        let (judged, moved) = outcome_with_free_places(&line, &free(&["G"]));
        assert_eq!((judged, moved), (Outcome::Pass, vec!["G".to_owned()]));
        // Without the evidence, or with another wallet's, the order fails.
        assert!(outcome_with_free_places(&line, &free(&[])).0.is_failure());
        assert!(
            outcome_with_free_places(&line, &free(&["B"]))
                .0
                .is_failure()
        );
        // Another wallet out of its place as well.
        let swapped = [("C", "cara"), ("B", "bert"), ("D", "dave"), ("G", "gina")];
        assert!(
            outcome_with_free_places(&search(&ours, &swapped), &free(&["G"]))
                .0
                .is_failure()
        );
        // A value that differs, or another wallet found.
        let renamed = [("B", "bert"), ("C", "cara"), ("D", "dave"), ("G", "gwen")];
        assert!(
            outcome_with_free_places(&search(&ours, &renamed), &free(&["G"]))
                .0
                .is_failure()
        );
        let other = [("B", "bert"), ("C", "cara"), ("D", "dave"), ("E", "gina")];
        assert!(
            outcome_with_free_places(&search(&ours, &other), &free(&["G", "E"]))
                .0
                .is_failure()
        );
        // The blocks and transactions found are judged as they are.
        let mut line = search(&ours, &theirs);
        if let Some(Reply::Answer { body, .. }) = &mut line.right {
            body["data"]["blocks"] = json!([{"id": "b"}]);
        }
        assert!(
            outcome_with_free_places(&line, &free(&["G"]))
                .0
                .is_failure()
        );
    }

    #[test]
    fn the_listing_differences_are_judged_by_what_they_change() {
        // A round's delegates: the same set, Heartwood's in the stored order.
        let a = json!({"publicKey": "02aa", "votes": "900"});
        let b = json!({"publicKey": "02bb", "votes": "1000"});
        let c = json!({"publicKey": "02cc", "votes": "900"});
        let round = |l: Value, r: Value| {
            outcome(&case(
                "/api/rounds/3/delegates",
                "roundOrder",
                answer(200, json!({"data": l})),
                answer(200, json!({"data": r})),
            ))
        };
        let order = ApiDifference::RoundOrder;
        assert!(is(&round(json!([b, a, c]), json!([b, c, a])), order));
        assert!(round(json!([a, b, c]), json!([b, c, a])).is_failure());
        assert!(round(json!([b, a]), json!([b, c, a])).is_failure());
        // A count the reference estimates: the same page, another count.
        let counted = |total: u64| {
            json!({"meta": {"count": 1, "totalCount": total,
            "pageCount": total, "totalCountIsEstimate": true}, "data": [a]})
        };
        let estimated = |l: Value, r: Value| {
            outcome(&case(
                "/api/blocks",
                "estimatedCount",
                answer(200, l),
                answer(200, r),
            ))
        };
        let estimate = ApiDifference::EstimatedCount;
        assert!(is(&estimated(counted(3), counted(9)), estimate));
        let other = json!({"meta": {"count": 1, "totalCount": 9, "pageCount": 9,
            "totalCountIsEstimate": true}, "data": [b]});
        assert!(estimated(counted(3), other).is_failure());
    }

    /// A document as `/api.json` answers it: `paths`, the type and group lists, a version.
    fn document(paths: &[&str], types: &[u64], groups: &[u64], version: &str) -> Value {
        let paths: serde_json::Map<String, Value> = (paths.iter())
            .map(|path| ((*path).to_owned(), json!({"get": {"summary": path}})))
            .collect();
        json!({
            "openapi": "3.0.0",
            "info": {"title": "API", "version": version},
            "paths": paths,
            "components": {"schemas": {
                "address": {"pattern": "^[D][a-km-zA-HJ-NP-Z1-9]{33}$"},
                "transactionTypes": {"enum": types},
                "transactionTypeGroups": {"enum": groups},
            }},
        })
    }

    #[test]
    fn the_document_differs_only_by_the_removed_routes_and_types() {
        let judged = |ours: Value, theirs: Value| {
            outcome(&case(
                "/api.json",
                "specKeptOnly",
                answer(200, ours),
                answer(200, theirs),
            ))
        };
        let kept = ApiDifference::SpecKeptOnly;
        let locks = ["/locks", "/locks/{id}", "/locks/unlocked"];
        let all: Vec<&str> = ["/blocks", "/votes"].into_iter().chain(locks).collect();
        let theirs = document(&all, &[0, 1, 2, 3, 6, 7, 8], &[1, 2], "4.3.1");
        let ours = document(
            &["/blocks", "/votes"],
            &[1, 2, 6, 7, 0],
            &[1, 2],
            "4.3.1-heartwood.0.1.0",
        );
        assert!(is(&judged(ours.clone(), theirs.clone()), kept));
        // The version names each node's build: alone, it is no difference.
        let mut same = theirs.clone();
        same["info"]["version"] = json!("4.3.1-heartwood.0.1.0");
        assert_eq!(judged(same, theirs.clone()), Outcome::Pass);
        // A served route left out, or a document without paths.
        let fewer = document(&["/blocks"], &[1, 2, 6, 7, 0], &[1, 2], "4.3.1");
        assert!(judged(fewer, theirs.clone()).is_failure());
        let none = document(&[], &[1, 2, 6, 7, 0], &[1, 2], "4.3.1");
        assert!(judged(none, theirs.clone()).is_failure());
        // A component patched otherwise, or a type list that is not the kept types.
        let mut pattern = ours.clone();
        pattern["components"]["schemas"]["address"]["pattern"] = json!("^[a][1-9]{33}$");
        assert!(judged(pattern, theirs.clone()).is_failure());
        let removed_type = document(
            &["/blocks", "/votes"],
            &[1, 2, 6, 7, 0, 8],
            &[1, 2],
            "4.3.1",
        );
        assert!(judged(removed_type, theirs.clone()).is_failure());
        let missing_type = document(&["/blocks", "/votes"], &[1, 2, 6, 0], &[1, 2], "4.3.1");
        assert!(judged(missing_type, theirs).is_failure());
    }

    #[test]
    fn the_fee_statistics_without_days_list_every_kept_type() {
        let zero = json!({"avg": "0", "burned": "0", "max": "0", "min": "0", "sum": "0"});
        let paid = json!({"avg": "5", "burned": "1", "max": "9", "min": "1", "sum": "10"});
        let fees = |ours: Value, theirs: Value, meta: Value| {
            outcome(&case(
                "/api/node/fees",
                "feeTypes",
                answer(200, json!({"meta": meta, "data": ours})),
                answer(200, json!({"meta": meta, "data": theirs})),
            ))
        };
        let types = ApiDifference::FeeTypes;
        let kept = json!({
            "1": {"secondSignature": zero, "delegateRegistration": zero, "transfer": paid,
                  "delegateResignation": zero},
            "2": {"burn": zero, "vote": zero},
        });
        let mut theirs = kept.clone();
        theirs["1"]["htlcLock"] = zero.clone();
        assert!(is(&fees(kept.clone(), theirs.clone(), json!({})), types));
        // A kept type left out, though its row holds zeros only.
        let mut fewer = kept.clone();
        fewer["2"]
            .as_object_mut()
            .map(|group| group.shift_remove("vote"));
        assert!(fees(fewer.clone(), theirs.clone(), json!({})).is_failure());
        // A removed type listed, with the reference's zeros, beside another it leaves out.
        let mut extra = theirs.clone();
        theirs["1"]["ipfs"] = zero;
        assert!(fees(extra.clone(), theirs.clone(), json!({})).is_failure());
        extra["1"]["ipfs"] = theirs["1"]["ipfs"].clone();
        assert_eq!(fees(extra, theirs, json!({})), Outcome::Pass);
        // With days, a type without transactions is listed by neither.
        let mut windowed = fewer.clone();
        windowed["1"]["transfer"] = paid.clone();
        let mut both = fewer;
        both["1"]["transfer"] = paid;
        both["1"]["htlcLock"] =
            json!({"avg": "0", "burned": "0", "max": "0", "min": "0", "sum": "0"});
        assert!(is(&fees(windowed, both, json!({"days": 7})), types));
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
        // The store keeps an amount for a burn only, zero included; none for a transfer (its
        // resource shows the sum of its transfers): those rows tie, whatever their resources show.
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
        // A burn of zero sorts by its zero, first from the lowest, where the rows without an
        // amount sort last.
        let zero = json!({"id": "z", "typeGroup": 2, "type": 0, "amount": "0"});
        assert_eq!(sort_key(&zero, "amount"), Some(json!("0")));
        assert!(judged(json!([zero, vote]), json!([vote, zero])).is_failure());
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
            judged(answer(500, failed.clone())),
            Outcome::Documented(r) if r == ApiDifference::StoreFailure.reason()
        ));
        // Any other answer of the node under test is a difference.
        let other = json!({"meta": {"count": 1, "totalCount": 1}, "data": [{"id": "x"}]});
        assert!(judged(answer(200, other.clone())).is_failure());
        // So is a 500 against any answer of the reference but the empty page.
        let against = |right: Reply| {
            outcome(&list(
                "memo=%25%5C",
                "storeFailure",
                answer(500, failed.clone()),
                right,
            ))
        };
        let two =
            json!({"meta": {"count": 2, "totalCount": 2}, "data": [{"id": "x"}, {"id": "y"}]});
        assert!(against(answer(200, two)).is_failure());
        assert!(
            against(answer(
                200,
                json!({"meta": {"totalCount": 0}, "data": [{}]})
            ))
            .is_failure()
        );
        assert!(against(answer(422, empty.clone())).is_failure());
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
            let short = (100, 100);
            headers(
                &with(left_vary),
                &with("origin"),
                Some(RouteId::PeersIndex),
                short,
                &mut differences,
            );
            let mut others = Vec::new();
            headers(
                &with(left_vary),
                &with("origin"),
                Some(RouteId::BlocksLast),
                short,
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
    fn a_listed_wallet_is_compared_as_a_wallet_alone_is() {
        let delegate = |version_first: bool, version: &str, produced: u64| {
            let mut delegate = serde_json::Map::new();
            delegate.insert("username".into(), json!("genesis_1"));
            delegate.insert("producedBlocks".into(), json!(produced));
            let (last, version) = (json!("8025a842"), json!(version));
            if version_first {
                delegate.insert("version".into(), version);
                delegate.insert("lastBlock".into(), last);
            } else {
                delegate.insert("lastBlock".into(), last);
                delegate.insert("version".into(), version);
            }
            json!({"address": "dA", "attributes": {"delegate": delegate}})
        };
        let judged = |path: &str, left: Value, right: Value| {
            let page = |item: Value| json!({"meta": {"count": 1}, "data": [item]});
            let (left, right) = (answer(200, page(left)), answer(200, page(right)));
            outcome(&case(path, "", left, right))
        };
        let (ours, theirs) = (delegate(false, "0.1.0", 3), delegate(true, "4.3.1", 3));
        for path in [
            "/api/wallets",
            "/api/wallets/top",
            "/api/delegates/genesis_1/voters",
        ] {
            assert_eq!(judged(path, ours.clone(), theirs.clone()), Outcome::Pass);
            assert!(judged(path, ours.clone(), delegate(true, "4.3.1", 4)).is_failure());
        }
        // The order of the listed wallets is still compared.
        let other = json!({"address": "dB"});
        let two = |items: Value| answer(200, json!({"meta": {"count": 2}, "data": items}));
        let swapped = case(
            "/api/wallets",
            "",
            two(json!([ours, other])),
            two(json!([other, theirs])),
        );
        assert!(outcome(&swapped).is_failure());
    }

    #[test]
    fn the_vary_of_an_answer_follows_its_own_length() {
        let fees = |types: Value| json!({"meta": {"days": 7}, "data": {"1": types}});
        let zero = json!({"avg": "0", "burned": "0", "max": "0", "min": "0", "sum": "0"});
        let ours = fees(json!({"transfer": zero}));
        let many: serde_json::Map<String, Value> = (0..20)
            .map(|n| (format!("type{n}"), zero.clone()))
            .chain([("transfer".to_owned(), zero.clone())])
            .collect();
        let theirs = fees(Value::Object(many));
        let judged = |left_vary: &str, left: &Value, right_vary: &str, right: &Value| {
            let reply =
                |vary: &str, body: &Value| with_header(answer(200, body.clone()), "vary", vary);
            outcome(&case(
                "/api/node/fees",
                "feeTypes",
                reply(left_vary, left),
                reply(right_vary, right),
            ))
        };
        // The reference's longer answer is compressed, Heartwood's shorter one is not.
        let compressed = "origin,accept-encoding";
        assert!(is(
            &judged("origin", &ours, compressed, &theirs),
            ApiDifference::FeeTypes
        ));
        // Equal headers are no difference; a vary that differs where its own answer's length
        // does not explain it is one.
        assert!(is(
            &judged(compressed, &ours, compressed, &theirs),
            ApiDifference::FeeTypes
        ));
        assert!(judged(compressed, &ours, "origin", &theirs).is_failure());
        assert!(judged("origin", &theirs, compressed, &theirs).is_failure());
    }

    #[test]
    fn an_estimated_count_excuses_the_previous_page_it_decides() {
        let meta = |total: u64, pages: u64, previous: Value| {
            json!({"meta": {"count": 0, "totalCount": total, "pageCount": pages,
                "totalCountIsEstimate": true, "previous": previous, "next": null,
                "self": "/blocks?page=3&limit=100"}, "data": []})
        };
        let judged = |left: Value, right: Value| {
            outcome(&case(
                "/api/blocks",
                "estimatedCount",
                answer(200, left),
                answer(200, right),
            ))
        };
        // Past the last page, the previous page follows the page count.
        let ours = meta(159, 2, json!("/blocks?page=2&limit=100"));
        assert!(is(
            &judged(ours.clone(), meta(78, 1, Value::Null)),
            ApiDifference::EstimatedCount
        ));
        let mut other = meta(78, 1, Value::Null);
        other["meta"]["self"] = json!("/blocks?page=4&limit=100");
        assert!(judged(ours, other).is_failure());
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
