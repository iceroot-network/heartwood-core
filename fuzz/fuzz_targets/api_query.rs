//! Fuzz the API's query grammar: a query string parsed as the reference's server parses it
//! (`parse_query`), validated against a route's query schema (`validate_query`), and the list
//! envelope written for a page of it (`envelope`), as a list route would answer it. On the state
//! listings, the validated query's criteria and order are also taken (`of_query`, `sorting_of`)
//! and tested against a wallet (`matches`), as the listing tests each wallet.
//!
//! No step may panic, and the work stays bounded whatever the query: the parse is the same each
//! time, at most 129 levels deep (the query and 128 below it), and the parse and the envelope are
//! written out within a size linear in the query's length, plus the grammar's budget of holes.

#![no_main]

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::schemas::{PARSED, parse_query, validate_query};
use heartwood_api::search::criteria::{of_query, sorting_of};
use heartwood_api::search::matches;
use heartwood_api::{ApiRequest, RouteId};
use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value, json};

/// The routes with a query schema, the path a request of each has, and whether it paginates; and
/// the routes without one, which take any query (the fee statistics' `days` is checked). The
/// listings' routes follow the first eleven, so that a choice byte keeps its route.
const ROUTES: [(RouteId, &str, bool); 27] = [
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
    (RouteId::BlockchainIndex, "/blockchain", false),
    (RouteId::BlockchainSearch, "/blockchain/search/abc", false),
    (RouteId::NodeFees, "/node/fees", false),
    (RouteId::TransactionsSchemas, "/transactions/schemas", false),
    (RouteId::DelegatesIndex, "/delegates", true),
    (RouteId::DelegatesShow, "/delegates/genesis_1", false),
    (
        RouteId::DelegatesVoters,
        "/delegates/genesis_1/voters",
        true,
    ),
    (RouteId::WalletsIndex, "/wallets", true),
    (RouteId::WalletsTop, "/wallets/top", true),
    (RouteId::BlocksIndex, "/blocks", true),
    (RouteId::BlocksMissed, "/blocks/missed", true),
    (RouteId::BlocksFirst, "/blocks/first", false),
    (
        RouteId::DelegatesBlocks,
        "/delegates/genesis_1/blocks",
        true,
    ),
    (
        RouteId::DelegatesMissedBlocks,
        "/delegates/genesis_1/blocks/missed",
        true,
    ),
    (RouteId::RoundsDelegates, "/rounds/12/delegates", false),
    (RouteId::VotesIndex, "/votes", true),
    (
        RouteId::VotesShow,
        "/votes/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        false,
    ),
    (
        RouteId::WalletsTransactionsSent,
        "/wallets/genesis_1/transactions/sent",
        true,
    ),
    (
        RouteId::WalletsTransactionsReceived,
        "/wallets/genesis_1/transactions/received",
        true,
    ),
    (RouteId::WalletsVotes, "/wallets/genesis_1/votes", true),
];

/// The state listings, whose handlers test each wallet or delegate against the query's criteria.
const STATE_LISTINGS: [RouteId; 4] = [
    RouteId::DelegatesIndex,
    RouteId::DelegatesVoters,
    RouteId::WalletsIndex,
    RouteId::WalletsTop,
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
    if STATE_LISTINGS.contains(&route) {
        // The order is read, and each item tested; an error is the reference's throw (a 500).
        let _ = sorting_of(&query, &[]);
        let wallet = json!({
            "address": "D6Z26L69gdk9qYmTv5uzk3uGepigtHY4ax", "publicKey": "02aa", "balance": "10",
            "nonce": "1", "attributes": {"delegate": {"username": "genesis_1", "rank": 1,
            "voteBalance": "5", "producedBlocks": 2}, "secondPublicKey": "03bb"},
            "votingFor": {"genesis_1": {"percent": 100, "votes": "10"}},
        });
        let _ = matches(&wallet, &[of_query(&query)]);
    }
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
