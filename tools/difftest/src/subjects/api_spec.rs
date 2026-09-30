//! The chain routes' documents: `api.spec` (the patched OpenAPI document), `api.transactionSchemas`
//! (the activated types' schemas), `api.feeStatistics` (the fee statistics of stored rows) and
//! `api.fuzzQuery` (a fuzz input of the query target, the route's choice and the raw query, through
//! the grammar, the route's validation and the pagination).

use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::{Pagination, envelope};
use heartwood_api::resources::fee_statistics;
use heartwood_api::schemas::{PARSED, parse_query, validate_query};
use heartwood_api::spec::{self, Patches};
use heartwood_api::{ApiRequest, RouteId};
use heartwood_crypto::utils::hex;
use heartwood_database::FeeStatisticsRow;
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.spec", spec),
    ("api.transactionSchemas", transaction_schemas),
    ("api.feeStatistics", fee_statistics),
    ("api.fuzzQuery", fuzz_query),
];

/// The routes of the query target, in the order of its choice byte: the route, the path of a
/// request to it (the reference's, with the base path) and whether it lists a page.
const ROUTES: [(RouteId, &str, bool); 11] = [
    (RouteId::TransactionsIndex, "/api/transactions", true),
    (
        RouteId::BlocksTransactions,
        "/api/blocks/12/transactions",
        true,
    ),
    (
        RouteId::WalletsTransactions,
        "/api/wallets/genesis_1/transactions",
        true,
    ),
    (RouteId::PeersIndex, "/api/peers", true),
    (
        RouteId::TransactionsUnconfirmed,
        "/api/transactions/unconfirmed",
        true,
    ),
    (RouteId::BlocksShow, "/api/blocks/12", false),
    (RouteId::NodeStatus, "/api/node/status", false),
    (RouteId::BlockchainIndex, "/api/blockchain", false),
    (
        RouteId::BlockchainSearch,
        "/api/blockchain/search/abc",
        false,
    ),
    (RouteId::NodeFees, "/api/node/fees", false),
    (
        RouteId::TransactionsSchemas,
        "/api/transactions/schemas",
        false,
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

/// A fuzz input of the query target: `{status: 500}` where the reference's plugins throw, the
/// route's refusal as `{status}`, else `{status: 200, query}` with the validated query the handler
/// reads and, for a route that lists a page, its `page` (the list's `meta`); `{}` for an empty
/// input.
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
    let (route, path, paginates) = ROUTES[usize::from(choice) % ROUTES.len()];
    let mut query = match validate_query(route, &parsed) {
        Ok(query) => query,
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
    out.insert("query".into(), query);
    if let Some(page) = page {
        out.insert("page".into(), page);
    }
    Ok(Verdict::Output(Value::Object(out)))
}
