//! The confirmed transaction routes: `GET /transactions` (the reference's criteria, the address
//! through the recipients side table, the implicit type group, the byte-wise memo pattern, the
//! asset containment; default order `timestamp:desc,sequence:desc`, ties broken by the unique key;
//! pagination) and `GET /transactions/{id}` (with its block; `confirmations` is the tip minus its
//! height plus one).
//!
//! The listing is shared: `GET /blocks/{id}/transactions` gives it the block's id, and the wallet
//! route `GET /wallets/{id}/transactions` the wallet's address ([`wallet_transactions`]).
//!
//! A read of the store runs on a blocking thread; a store error, the read's 3 s timeout included,
//! is logged and answered with a 500, where the reference answers a list with an empty page and
//! logs nothing.

use std::sync::Arc;

use heartwood_crypto::utils::js::is_truthy;
use heartwood_crypto::{Milestones, Params, Transaction, TransactionId};
use heartwood_database::filters::{CastLimit, asset_alternatives};
use heartwood_database::{
    DatabaseError, Direction, Numeric, Order, Page, Reader, TransactionCriteria, TransactionFilter,
    TransactionRecord, TransactionRow,
};
use serde_json::{Map, Value};

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};
use crate::resources::{self, TransactionBlock};

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::TransactionsIndex,
            handler: |ctx, request| Box::pin(index(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
    ]
}

/// The transactions of the criteria.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let criteria = criteria(&request.query)?;
    listing(&ctx, &request, criteria, Shape::WithBlock).await
}

/// A transaction by id.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let not_found = || ApiError::not_found("Transaction not found");
    // The reference matches the id as a prefix of the stored lowercase id, so only a lowercase
    // id finds it.
    let id = request
        .param("id")
        .filter(|id| is_lowercase_hex(id))
        .and_then(|id| TransactionId::from_hex(id).ok())
        .ok_or_else(not_found)?;
    let record = read(&ctx, move |reader| reader.transaction(&id))
        .await?
        .ok_or_else(not_found)?;
    let state = ctx.state();
    let shape = Shape::WithBlock;
    let data = present(
        &record.row,
        shape,
        transform(&request.query),
        state.height(),
        &ctx,
    )?;
    Ok(ApiReply::data(data))
}

/// The history part of `GET /wallets/{id}/transactions`: the transactions of the query's criteria
/// whose sender or recipient is `address`, as `GET /transactions` answers them. The wallet route
/// finds the wallet first, with its own 404 and 422 answers, and gives its address.
pub async fn wallet_transactions(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
    address: String,
) -> Result<ApiReply, ApiError> {
    let criteria = criteria(&request.query)?.map(|criteria| TransactionCriteria {
        address: vec![address],
        ..criteria
    });
    listing(&ctx, &request, criteria, Shape::WithBlock).await
}

/// The resource a transaction route shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// The reference's `TransactionWithBlockResource`: its block's timestamp, and its
    /// confirmations.
    WithBlock,
    /// The reference's `TransactionResource`: no timestamp, and `confirmations: 0`.
    Plain,
}

/// The listing of `request` for `criteria` (`None`: criteria that match nothing), as `shape`.
pub(crate) async fn listing(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    criteria: Option<TransactionCriteria>,
    shape: Shape,
) -> Result<ApiReply, ApiError> {
    let pagination = Pagination::of(&request.query);
    let page = Page {
        offset: pagination.offset,
        limit: pagination.limit,
    };
    let order = order(&request.query);
    let (rows, total) = match criteria {
        Some(criteria) => {
            let filter = TransactionFilter::of(criteria);
            let listing = read(ctx, move |reader| {
                reader.transactions(&filter, &order, page)
            })
            .await?;
            (listing.rows, listing.total_count)
        }
        None => (Vec::new(), 0),
    };
    let tip = ctx.state().height();
    let transform = transform(&request.query);
    let data = rows
        .iter()
        .map(|record: &TransactionRecord| present(&record.row, shape, transform, tip, ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let body = envelope(
        request,
        pagination,
        total,
        ctx.config.estimate_total_count,
        data,
    );
    Ok(ApiReply::ok(body))
}

/// Run `query` on a reader, on a blocking thread; a store error is logged and answered with 500.
pub(crate) async fn read<T: Send + 'static>(
    ctx: &Arc<ApiContext>,
    query: impl FnOnce(&Reader<'_>) -> Result<T, DatabaseError> + Send + 'static,
) -> Result<T, ApiError> {
    let history = Arc::clone(&ctx.history);
    match tokio::task::spawn_blocking(move || query(&history.get())).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            heartwood_kernel::error!("A read of the store for the API failed: {error}");
            Err(ApiError::internal())
        }
        Err(error) => {
            heartwood_kernel::error!("A read of the store for the API did not finish: {error}");
            Err(ApiError::internal())
        }
    }
}

/// The stored transaction `row` as the route shows it: the resource of `shape` when `transform`,
/// otherwise the reference's raw data.
pub(crate) fn present(
    row: &TransactionRow,
    shape: Shape,
    transform: bool,
    tip: u32,
    ctx: &ApiContext,
) -> Result<Value, ApiError> {
    let milestones: &Milestones = &ctx.node.crypto.milestones;
    let params = milestones.at(row.block_height);
    let transaction = decode(row, params)?;
    if !transform {
        return Ok(raw(&transaction, row, params));
    }
    let block = TransactionBlock {
        id: row.block_id,
        height: row.block_height,
        timestamp: match shape {
            Shape::WithBlock => Some(row.timestamp),
            Shape::Plain => None,
        },
        confirmations: match shape {
            // The tip minus the block's height plus one, as the reference counts them.
            Shape::WithBlock => tip.saturating_sub(row.block_height).saturating_add(1),
            Shape::Plain => 0,
        },
    };
    Ok(resources::transaction(
        &transaction,
        Some(&block),
        milestones,
    ))
}

/// The stored transaction of `row`, read from its bytes as the reference's `fromBytesUnsafe` reads
/// them; the node verified them before it stored them.
pub(crate) fn decode(row: &TransactionRow, params: &Params) -> Result<Transaction, ApiError> {
    Transaction::decode_trusted(&row.serialised, params).map_err(|error| {
        heartwood_kernel::error!(
            "The stored transaction {} cannot be read: {error}",
            row.id.to_hex()
        );
        ApiError::internal()
    })
}

/// A confirmed transaction's raw data, as the reference's model converter makes it: the fields
/// read from its bytes (the version byte as it is, the type group always), then its nonce, burned
/// fee, block id, block height and sequence from its row.
fn raw(transaction: &Transaction, row: &TransactionRow, params: &Params) -> Value {
    let Value::Object(json) = transaction.to_json(params) else {
        return Value::Null;
    };
    let data = transaction.data();
    let mut out = Map::new();
    for (key, value) in json {
        let group = key == "network" && !out.contains_key("typeGroup");
        match key.as_str() {
            "version" => out.insert(key, Value::from(data.version)),
            _ => out.insert(key, value),
        };
        if group {
            out.insert("typeGroup".into(), Value::from(data.kind.type_group()));
        }
    }
    out.insert("nonce".into(), Value::from(row.nonce.to_string()));
    out.insert("burnedFee".into(), Value::from(row.burned_fee.to_string()));
    out.insert("blockId".into(), Value::from(row.block_id.to_hex()));
    out.insert("blockHeight".into(), Value::from(row.block_height));
    out.insert("sequence".into(), Value::from(row.sequence));
    Value::Object(out)
}

/// Whether the validated query asks for the transformed resource (`transform`, true by default).
pub(crate) fn transform(query: &Value) -> bool {
    query.get("transform").is_some_and(is_truthy)
}

/// The order of the validated query's `orderBy`, as the reference's controllers read it: a text is
/// split at commas, a list is taken as it is, and each key is `property:direction`, where any
/// direction but `desc` is ascending.
pub(crate) fn order(query: &Value) -> Vec<Order> {
    let keys: Vec<&str> = match query.get("orderBy") {
        Some(Value::String(text)) if !text.is_empty() => text.split(',').collect(),
        Some(Value::Array(items)) => items
            .iter()
            .flat_map(|item| match item {
                Value::Array(inner) => inner.iter().filter_map(Value::as_str).collect(),
                other => other.as_str().into_iter().collect::<Vec<_>>(),
            })
            .collect(),
        _ => Vec::new(),
    };
    keys.into_iter()
        .map(|key| {
            let mut parts = key.split(':');
            Order {
                property: parts.next().unwrap_or_default().to_owned(),
                direction: match parts.next() {
                    Some("desc") => Direction::Desc,
                    _ => Direction::Asc,
                },
            }
        })
        .collect()
}

/// The transaction criteria of the validated query `query`; `None` when a key's list of values is
/// empty, which matches nothing, as the reference's filter makes an empty or-list false. The keys
/// that are no criteria (the order, the page, `transform`) are left out.
pub(crate) fn criteria(query: &Value) -> Result<Option<TransactionCriteria>, ApiError> {
    let mut criteria = TransactionCriteria::default();
    let Some(query) = query.as_object() else {
        return Ok(Some(criteria));
    };
    for (key, value) in query {
        let values: &[Value] = match value {
            Value::Array(values) => values,
            one => std::slice::from_ref(one),
        };
        let criterion_key = matches!(
            key.as_str(),
            "address"
                | "senderId"
                | "recipientId"
                | "id"
                | "version"
                | "blockHeight"
                | "blockId"
                | "sequence"
                | "timestamp"
                | "nonce"
                | "senderPublicKey"
                | "type"
                | "typeGroup"
                | "memo"
                | "amount"
                | "fee"
                | "burnedFee"
                | "asset"
        );
        if criterion_key && values.is_empty() {
            return Ok(None);
        }
        match key.as_str() {
            "address" => criteria.address = texts(key, values)?,
            "senderId" => criteria.sender_id = texts(key, values)?,
            "recipientId" => criteria.recipient_id = texts(key, values)?,
            "id" => criteria.id = texts(key, values)?,
            "version" => criteria.version = integers(key, values)?,
            "blockHeight" => criteria.block_height = numerics(key, values)?,
            "blockId" => criteria.block_id = texts(key, values)?,
            "sequence" => criteria.sequence = numerics(key, values)?,
            "timestamp" => criteria.timestamp = numerics(key, values)?,
            "nonce" => criteria.nonce = numerics(key, values)?,
            "senderPublicKey" => criteria.sender_public_key = texts(key, values)?,
            "type" => criteria.kind = integers(key, values)?,
            "typeGroup" => criteria.type_group = integers(key, values)?,
            "memo" => {
                criteria.memo = texts(key, values)?
                    .into_iter()
                    .map(String::into_bytes)
                    .collect();
            }
            "amount" => criteria.amount = numerics(key, values)?,
            "fee" => criteria.fee = numerics(key, values)?,
            "burnedFee" => criteria.burned_fee = numerics(key, values)?,
            "asset" => {
                for value in values {
                    criteria
                        .asset
                        .extend(asset_alternatives(value).map_err(cast_limit)?);
                }
            }
            _ => {}
        }
    }
    Ok(Some(criteria))
}

/// The reference's filter throws past its limit of casts, and the request fails with a 500.
fn cast_limit(error: CastLimit) -> ApiError {
    heartwood_kernel::error!("A transaction criteria of the API was refused: {error}");
    ApiError::internal()
}

/// A criteria value of an unexpected shape: the validated query never has one.
fn invalid(key: &str) -> ApiError {
    ApiError::unprocessable(format!("\"{key}\" is not valid"))
}

/// The values of `key` as texts.
fn texts(key: &str, values: &[Value]) -> Result<Vec<String>, ApiError> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(key))
        })
        .collect()
}

/// The values of `key` as whole numbers from 0.
fn integers(key: &str, values: &[Value]) -> Result<Vec<u64>, ApiError> {
    values
        .iter()
        .map(|value| integer(value).ok_or_else(|| invalid(key)))
        .collect()
}

/// The values of `key` as numbers or ranges (`{from, to}`, either end open).
fn numerics(key: &str, values: &[Value]) -> Result<Vec<Numeric>, ApiError> {
    values
        .iter()
        .map(|value| match value {
            Value::Object(range) => {
                let end = |name: &str| match range.get(name) {
                    None => Ok(None),
                    Some(end) => integer(end).map(Some).ok_or_else(|| invalid(key)),
                };
                Ok(Numeric::Range {
                    from: end("from")?,
                    to: end("to")?,
                })
            }
            one => integer(one).map(Numeric::Equal).ok_or_else(|| invalid(key)),
        })
        .collect()
}

/// A whole number from 0, as a JSON number or a text of digits.
fn integer(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// Whether `text` is lowercase hexadecimal, as the store writes ids.
pub(crate) fn is_lowercase_hex(text: &str) -> bool {
    text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_validated_query_gives_its_criteria_and_leaves_out_the_rest() {
        let query = json!({
            "address": "DA",
            "senderId": ["DB", "DC"],
            "type": 6,
            "typeGroup": ["1"],
            "blockHeight": {"from": 2},
            "amount": [1, {"to": 5}, {"from": 3, "to": 4}],
            "memo": "x%",
            "asset": {"votes": {"alice": "1"}},
            "orderBy": ["timestamp:desc"],
            "page": 1,
            "limit": 100,
            "offset": 0,
            "transform": true,
        });
        let expected = TransactionCriteria {
            address: vec!["DA".into()],
            sender_id: vec!["DB".into(), "DC".into()],
            kind: vec![6],
            type_group: vec![1],
            block_height: vec![Numeric::Range {
                from: Some(2),
                to: None,
            }],
            amount: vec![
                Numeric::Equal(1),
                Numeric::Range {
                    from: None,
                    to: Some(5),
                },
                Numeric::Range {
                    from: Some(3),
                    to: Some(4),
                },
            ],
            memo: vec![b"x%".to_vec()],
            asset: vec![
                json!({"votes": {"alice": "1"}}),
                json!({"votes": {"alice": 1.0}}),
            ],
            ..TransactionCriteria::default()
        };
        assert_eq!(criteria(&query), Ok(Some(expected)));
    }

    #[test]
    fn an_empty_list_matches_nothing_and_a_wrong_shape_is_refused() {
        assert_eq!(criteria(&json!({"address": []})), Ok(None));
        assert_eq!(
            criteria(&json!({"orderBy": []})),
            Ok(Some(TransactionCriteria::default()))
        );
        assert_eq!(
            criteria(&json!({"fee": "-1"})).map_err(|e| e.status),
            Err(422)
        );
        assert_eq!(
            criteria(&json!({"memo": 5})).map_err(|e| e.status),
            Err(422)
        );
        let casts = json!({"asset": {"a": "1", "b": "2", "c": "3", "d": "4", "e": "5", "f": "6"}});
        assert_eq!(criteria(&casts).map_err(|e| e.status), Err(500));
    }

    #[test]
    fn the_order_is_read_as_the_reference_reads_it() {
        let order_of = |value: Value| order(&json!({ "orderBy": value }));
        let key = |property: &str, direction| Order {
            property: property.into(),
            direction,
        };
        assert_eq!(
            order_of(json!(["timestamp:desc", "sequence:desc"])),
            [
                key("timestamp", Direction::Desc),
                key("sequence", Direction::Desc)
            ]
        );
        assert_eq!(
            order_of(json!("amount:asc,fee:DESC,nonce")),
            [
                key("amount", Direction::Asc),
                key("fee", Direction::Asc),
                key("nonce", Direction::Asc)
            ]
        );
        assert_eq!(order_of(json!("")), []);
        assert_eq!(order(&json!({})), []);
    }

    #[test]
    fn transform_is_the_query_value_as_javascript_tests_it() {
        assert!(transform(&json!({"transform": true})));
        assert!(!transform(&json!({"transform": false})));
        assert!(!transform(&json!({})));
    }
}
