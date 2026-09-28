//! Fuzz the API's query grammar: a query string parsed as the reference's server parses it
//! (`parse_query`), validated against a route's query schema (`validate_query`), and the list
//! envelope written for a page of it (`envelope`), as a list route would answer it.
//!
//! No step may panic, and the work stays bounded whatever the query: the parse is the same each
//! time, at most 129 levels deep (the query and 128 below it), and the parse and the envelope are
//! written out within a size linear in the query's length, plus the grammar's budget of holes.

#![no_main]

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::schemas::{PARSED, parse_query, validate_query};
use heartwood_api::{ApiRequest, RouteId};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value};

/// The routes with a query schema, the path a request of each has, and whether it paginates; and
/// one route without a schema, which takes any query.
const ROUTES: [(RouteId, &str, bool); 7] = [
    (RouteId::TransactionsIndex, "/transactions", true),
    (RouteId::BlocksTransactions, "/blocks/12/transactions", true),
    (
        RouteId::WalletsTransactions,
        "/wallets/genesis_1/transactions",
        true,
    ),
    (RouteId::PeersIndex, "/peers", true),
    (
        RouteId::TransactionsUnconfirmed,
        "/transactions/unconfirmed",
        true,
    ),
    (RouteId::BlocksShow, "/blocks/12", false),
    (RouteId::NodeStatus, "/node/status", false),
];

/// The most holes of sparse lists a parse is written out with (the grammar's own bound).
const HOLES: usize = 16_384;

/// The levels of objects and lists in `value`, counted without recursion.
fn depth(value: &Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(value, 1)];
    while let Some((value, level)) = stack.pop() {
        deepest = deepest.max(level);
        match value {
            Value::Array(items) => stack.extend(items.iter().map(|item| (item, level + 1))),
            Value::Object(map) => stack.extend(map.values().map(|item| (item, level + 1))),
            _ => {}
        }
    }
    deepest
}

fuzz_target!(|data: &[u8]| {
    let Some((&choice, rest)) = data.split_first() else {
        return;
    };
    let raw = String::from_utf8_lossy(rest);
    let parsed = parse_query(&raw);
    assert_eq!(parsed, parse_query(&raw), "the parse is the same each time");
    let written = serde_json::to_string(&parsed).unwrap_or_default().len();
    let bound = 16 * raw.len() + 6 * HOLES + 4096;
    assert!(
        written <= bound,
        "a parse of {} bytes is {written}",
        raw.len()
    );
    let Value::Object(_) = &parsed else {
        // The reference's plugins throw on this query: its server answers 500 on every path.
        assert!(parsed.is_null());
        return;
    };
    assert!(depth(&parsed) <= 129, "the parse is deeper than 129 levels");
    let (route, path, paginates) = ROUTES[usize::from(choice) % ROUTES.len()];
    let Ok(query) = validate_query(route, &parsed) else {
        return;
    };
    assert_eq!(query.get(PARSED).is_some(), paginates);
    if !paginates {
        return;
    }
    let pagination = Pagination::of(&query);
    let total = u64::from(choice) * 37;
    let count = pagination
        .limit
        .min(total.saturating_sub(pagination.offset));
    let request = ApiRequest {
        route,
        params: Map::new(),
        path: path.to_owned(),
        query,
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    let data = vec![Value::Null; usize::try_from(count).unwrap_or(0)];
    let body = envelope(&request, pagination, total, false, data);
    let written = serde_json::to_string(&body).unwrap_or_default().len();
    let bound = 5 * (4 * bound + 1024) + 4096;
    assert!(
        written <= bound,
        "an envelope of {} bytes is {written}",
        raw.len()
    );
});
