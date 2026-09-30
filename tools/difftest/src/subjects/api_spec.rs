//! The chain routes' documents: `api.spec` (the patched OpenAPI document), `api.transactionSchemas`
//! (the activated types' schemas), `api.feeStatistics` (the fee statistics of stored rows) and
//! `api.fuzzQuery` (a fuzz input of the query target, the route's choice and the raw query, through
//! the grammar, the route's validation and the pagination).

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::resources::fee_statistics;
use heartwood_api::schemas::{BEYOND, PARSED, parse_query, validate_query};
use heartwood_api::spec::{self, Patches};
use heartwood_api::{ApiRequest, RouteId};
use heartwood_crypto::utils::hex;
use heartwood_database::FeeStatisticsRow;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed};
use crate::subject::{ApiDifference, Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.spec", spec),
    ("api.transactionSchemas", transaction_schemas),
    ("api.feeStatistics", fee_statistics),
    ("api.fuzzQuery", fuzz_query),
];

/// The routes of the query target, in the order of its choice byte: the route, the path of a
/// request to it (the reference's, with the base path), whether it lists a page and whether its
/// handler reads the query (the reference validates none on the others, so no handler looks at
/// what the plugins wrote). The listings' routes follow the first eleven.
const ROUTES: [(RouteId, &str, bool, bool); 27] = [
    (RouteId::TransactionsIndex, "/api/transactions", true, true),
    (
        RouteId::BlocksTransactions,
        "/api/blocks/12/transactions",
        true,
        true,
    ),
    (
        RouteId::WalletsTransactions,
        "/api/wallets/genesis_1/transactions",
        true,
        true,
    ),
    (RouteId::PeersIndex, "/api/peers", true, true),
    (
        RouteId::TransactionsUnconfirmed,
        "/api/transactions/unconfirmed",
        true,
        true,
    ),
    (RouteId::BlocksShow, "/api/blocks/12", false, true),
    (RouteId::NodeStatus, "/api/node/status", false, false),
    (RouteId::BlockchainIndex, "/api/blockchain", false, false),
    (
        RouteId::BlockchainSearch,
        "/api/blockchain/search/abc",
        false,
        false,
    ),
    (RouteId::NodeFees, "/api/node/fees", false, true),
    (
        RouteId::TransactionsSchemas,
        "/api/transactions/schemas",
        false,
        false,
    ),
    (RouteId::DelegatesIndex, "/api/delegates", true, true),
    (
        RouteId::DelegatesShow,
        "/api/delegates/genesis_1",
        false,
        false,
    ),
    (
        RouteId::DelegatesVoters,
        "/api/delegates/genesis_1/voters",
        true,
        true,
    ),
    (RouteId::WalletsIndex, "/api/wallets", true, true),
    (RouteId::WalletsTop, "/api/wallets/top", true, true),
    (RouteId::BlocksIndex, "/api/blocks", true, true),
    (RouteId::BlocksMissed, "/api/blocks/missed", true, true),
    (RouteId::BlocksFirst, "/api/blocks/first", false, true),
    (
        RouteId::DelegatesBlocks,
        "/api/delegates/genesis_1/blocks",
        true,
        true,
    ),
    (
        RouteId::DelegatesMissedBlocks,
        "/api/delegates/genesis_1/blocks/missed",
        true,
        true,
    ),
    (
        RouteId::RoundsDelegates,
        "/api/rounds/12/delegates",
        false,
        false,
    ),
    (RouteId::VotesIndex, "/api/votes", true, true),
    (
        RouteId::VotesShow,
        "/api/votes/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        false,
        true,
    ),
    (
        RouteId::WalletsTransactionsSent,
        "/api/wallets/genesis_1/transactions/sent",
        true,
        true,
    ),
    (
        RouteId::WalletsTransactionsReceived,
        "/api/wallets/genesis_1/transactions/received",
        true,
        true,
    ),
    (
        RouteId::WalletsVotes,
        "/api/wallets/genesis_1/votes",
        true,
        true,
    ),
];

/// A whole number of `input[key]`.
fn number<T: TryFrom<i64>>(value: &Value, what: &str) -> Outcome<T> {
    value
        .as_i64()
        .and_then(|number| T::try_from(number).ok())
        .ok_or_else(|| failed(format!("input.{what} is not a whole number in range")))
}

/// The decimal text `value` holds, as the repository returns a `bigint`.
fn amount<T: std::str::FromStr>(value: &Value, what: &str) -> Outcome<T> {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| failed(format!("{what} is not a decimal number")))
}

fn spec(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let text = |value: &Value, what: &str| {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| failed(format!("input.{what} is not text")))
    };
    let patches = Patches {
        pub_key_hash: number(&input["network"]["pubKeyHash"], "network.pubKeyHash")?,
        max_transactions_per_request: number(
            &input["pool"]["maxTransactionsPerRequest"],
            "pool.maxTransactionsPerRequest",
        )?,
        base_path: text(&input["basePath"], "basePath")?,
        version: text(&input["version"], "version")?,
        missed_blocks_lookback: number(
            &input["blockchain"]["missedBlocksLookback"],
            "blockchain.missedBlocksLookback",
        )?,
    };
    let document = spec::document(&patches).ok_or_else(|| failed("the document is not served"))?;
    Ok(Verdict::Output(json!({ "document": document })))
}

fn transaction_schemas(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    let reply = spec::transaction_schemas().map_err(|error| failed(error.to_string()))?;
    Ok(Verdict::Output(reply.body))
}

fn fee_statistics(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let list = case.input["rows"]
        .as_array()
        .ok_or_else(|| failed("input.rows is not a list"))?;
    let mut rows = Vec::with_capacity(list.len());
    for row in list {
        rows.push(FeeStatisticsRow {
            type_group: number(&row["typeGroup"], "rows.typeGroup")?,
            kind: number(&row["type"], "rows.type")?,
            avg: amount(&row["avg"], "rows.avg")?,
            min: amount(&row["min"], "rows.min")?,
            max: amount(&row["max"], "rows.max")?,
            sum: amount(&row["sum"], "rows.sum")?,
            burned: amount(&row["burned"], "rows.burned")?,
        });
    }
    let days = match &case.input["days"] {
        Value::Null => None,
        days => Some(number(days, "days")?),
    };
    Ok(Verdict::Output(fee_statistics::response(days, &rows)))
}

/// The deepest query written out whole; the JSON readers of the harness stop at 128 levels.
const MOST_DEPTH: usize = 100;

/// How many levels of objects and lists `value` has.
fn depth(value: &Value) -> usize {
    match value {
        Value::Object(members) => 1 + members.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Whether `route` refuses the query `parsed` for a value the grammar wrote out shortened, past its
/// bounds, alone: the query without the grammar's note of the shortened keys is valid.
fn beyond_the_bounds(route: RouteId, parsed: &Value) -> bool {
    let mut whole = parsed.clone();
    let noted = whole
        .as_object_mut()
        .and_then(|map| map.shift_remove(BEYOND))
        .is_some();
    noted && validate_query(route, &whole).is_ok()
}

/// A fuzz input of the query target: `{status: 500}` where the reference's plugins throw, the
/// route's refusal as `{status}`, else `{status: 200, query}` with the validated query the handler
/// reads and, for a route that lists a page, its `page` (the list's `meta`); `{}` for an empty
/// input. A refusal for a value past the grammar's bounds alone is Heartwood's by design
/// ([`ApiDifference::QueryBounds`]).
fn fuzz_query(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let text = case.input["hex"]
        .as_str()
        .ok_or_else(|| failed("input.hex is not text"))?;
    let bytes = hex::decode(text).map_err(|error| failed(format!("input.hex: {error}")))?;
    let Some((&choice, rest)) = bytes.split_first() else {
        return Ok(Verdict::Output(json!({})));
    };
    let status = |status: u16| Ok(Verdict::Output(json!({ "status": status })));
    let parsed = parse_query(&String::from_utf8_lossy(rest));
    if parsed.is_null() {
        return status(500);
    }
    let (route, path, paginates, reads) = ROUTES[usize::from(choice) % ROUTES.len()];
    let mut query = match validate_query(route, &parsed) {
        Ok(query) => query,
        Err(error) if error.status == 500 && beyond_the_bounds(route, &parsed) => {
            return Ok(Verdict::RefusedByDesign {
                difference: Documented::Api(ApiDifference::QueryBounds),
                holds: true,
            });
        }
        Err(error) => return status(error.status),
    };
    let mut page = None;
    if paginates {
        let pagination = Pagination::of(&query);
        let total = u64::from(choice) * 37;
        let count = pagination
            .limit
            .min(total.saturating_sub(pagination.offset));
        let request = ApiRequest {
            route,
            params: Map::new(),
            path: path.to_owned(),
            query: query.clone(),
            payload: None,
            client: IpAddr::V4(Ipv4Addr::LOCALHOST),
        };
        let data = vec![Value::Null; usize::try_from(count).unwrap_or(0)];
        let body = envelope(&request, pagination, total, false, data);
        page = Some(body["meta"].clone());
    }
    if let Some(map) = query.as_object_mut() {
        map.shift_remove(PARSED);
    }
    let mut out = Map::new();
    out.insert("status".into(), json!(200));
    let levels = depth(&query);
    if reads {
        let written = if levels > MOST_DEPTH {
            json!({ "depth": levels })
        } else {
            query
        };
        out.insert("query".into(), written);
    }
    if let Some(page) = page {
        out.insert("page".into(), page);
    }
    Ok(Verdict::Output(Value::Object(out)))
}
