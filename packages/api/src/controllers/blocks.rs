//! The block routes: `GET /blocks/last` (the tip), `GET /blocks/{id}` (a full id or a height; a
//! height above the tip is 404 "Block not found") and `GET /blocks/{id}/transactions` (the block's
//! transactions with the transaction criteria and pagination, each without its `timestamp` and with
//! `confirmations: 0`; 404 unless the id is a full id or a height at or below the tip).
//!
//! `{id}` is read as the reference reads it: when JavaScript's `Number` of it is a height from 1
//! to the tip, it is that height (so a whole id of decimal digits can be a height too); otherwise
//! it is an id. The reference then matches the id as a prefix, which finds an unrelated block for
//! a height above the tip; here only a whole id, as stored, finds its block.

use std::sync::Arc;

use heartwood_crypto::BlockId;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::transactions::types::Asset;
use heartwood_database::filters::js_number;
use heartwood_database::{
    BlockCriteria, BlockFilter, BlockRow, BlockSelector, CountFlag, MissedBlockCriteria,
    MissedBlockFilter, Numeric, Page, TransactionCriteria,
};
use serde_json::{Map, Value};

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::controllers::transactions::{
    Shape, criteria, decode, integers, is_lowercase_hex, listing, numerics, order, read, texts,
    transform,
};
use crate::errors::ApiError;
use crate::pagination::{Pagination, envelope};
use crate::resources;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::BlocksLast,
            handler: |ctx, request| Box::pin(last(ctx, request)),
        },
        Route {
            id: RouteId::BlocksShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
        Route {
            id: RouteId::BlocksTransactions,
            handler: |ctx, request| Box::pin(transactions(ctx, request)),
        },
    ]
}

/// The tip.
pub async fn last(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let tip = ctx.state().height();
    block(&ctx, &request, BlockSelector::Height(tip), tip).await
}

/// A block by id or height.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let tip = ctx.state().height();
    let selector = selector(&request, tip).ok_or_else(not_found)?;
    block(&ctx, &request, selector, tip).await
}

/// A block's transactions.
pub async fn transactions(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let tip = ctx.state().height();
    let selector = selector(&request, tip).ok_or_else(not_found)?;
    let row = read(&ctx, move |reader| reader.block(selector))
        .await?
        .ok_or_else(not_found)?;
    // The query's criteria, with the block's id in place of any `blockId` it gives.
    let criteria = criteria(&request.query)?.map(|criteria| TransactionCriteria {
        block_id: vec![row.id.to_hex()],
        ..criteria
    });
    listing(&ctx, &request, criteria, Shape::Plain).await
}

/// 404 "Block not found".
fn not_found() -> ApiError {
    ApiError::not_found("Block not found")
}

/// The block that `{id}` names, if it can name a stored one.
fn selector(request: &ApiRequest, tip: u32) -> Option<BlockSelector> {
    let id = request.param("id")?;
    let height = js_number(id);
    if height != 0.0 && height <= f64::from(tip) {
        // A height: whole from 1 and at most the tip, so the conversion is exact; a fraction or
        // a number below 1 names no block.
        let whole = height >= 1.0 && height.fract() == 0.0;
        return whole.then_some(BlockSelector::Height(height as u32));
    }
    (id.len() == 64 && is_lowercase_hex(id))
        .then(|| BlockId::from_hex(id).ok().map(BlockSelector::Id))
        .flatten()
}

/// The block `selector` names: the reference's block resource, or its raw row when the query's
/// `transform` is false.
async fn block(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    selector: BlockSelector,
    tip: u32,
) -> Result<ApiReply, ApiError> {
    let transform = transform(&request.query);
    let (row, transfers) = read(ctx, move |reader| {
        let Some(row) = reader.block(selector)? else {
            return Ok((None, Vec::new()));
        };
        let transfers = if transform {
            reader
                .block_transactions(&row.id)?
                .into_iter()
                .filter(|record| (record.row.type_group, record.row.kind) == (1, 6))
                .map(|record| record.row)
                .collect()
        } else {
            Vec::new()
        };
        Ok((Some(row), transfers))
    })
    .await?;
    let row = row.ok_or_else(not_found)?;
    if !transform {
        return Ok(ApiReply::data(raw(&row)));
    }
    let milestones = &ctx.node.crypto.milestones;
    let mut transferred = 0_u128;
    for transfer in &transfers {
        let transaction = decode(transfer, milestones.at(transfer.block_height))?;
        if let Asset::Transfer { transfers } = &transaction.data().asset {
            transferred += transfers
                .iter()
                .map(|item| u128::from(item.amount))
                .sum::<u128>();
        }
    }
    // The amount forged is the block's total amount and every amount its transfers carry, which
    // the block's row alone does not hold.
    Ok(ApiReply::data(resources::block(
        &row,
        transferred,
        tip,
        milestones,
    )))
}

/// A block's raw data, as the reference's block model holds it: its columns in the model's
/// order, amounts as decimal texts, the donations object, and no `username` when it has none.
pub(crate) fn raw(row: &BlockRow) -> Value {
    let mut out = Map::new();
    let donations = serde_json::from_str::<Value>(&row.donations).unwrap_or(Value::Null);
    out.insert("id".into(), Value::from(row.id.to_hex()));
    out.insert("version".into(), Value::from(row.version));
    out.insert("timestamp".into(), Value::from(row.timestamp));
    out.insert(
        "previousBlock".into(),
        row.previous_block
            .map_or(Value::Null, |id| Value::from(id.to_hex())),
    );
    out.insert("height".into(), Value::from(row.height));
    out.insert(
        "numberOfTransactions".into(),
        Value::from(row.number_of_transactions),
    );
    out.insert(
        "totalAmount".into(),
        Value::from(row.total_amount.to_string()),
    );
    out.insert("totalFee".into(), Value::from(row.total_fee.to_string()));
    out.insert("burnedFee".into(), Value::from(row.burned_fee.to_string()));
    out.insert("reward".into(), Value::from(row.reward.to_string()));
    out.insert("donations".into(), donations);
    out.insert("payloadLength".into(), Value::from(row.payload_length));
    out.insert("payloadHash".into(), Value::from(row.payload_hash.as_str()));
    out.insert(
        "generatorPublicKey".into(),
        Value::from(row.generator_public_key.as_str()),
    );
    if let Some(username) = &row.username {
        out.insert("username".into(), Value::from(username.as_str()));
    }
    out.insert(
        "blockSignature".into(),
        Value::from(row.block_signature.as_str()),
    );
    Value::Object(out)
}

/// `GET /blocks`: the blocks matching the criteria, sorted and paginated. Without `transform` the
/// reference reads with no listing options, so it reports an estimate (`totalCountIsEstimate:
/// true`), or `false` when its store refuses the criteria and the listing fails; the count here is
/// exact either way.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let transform = transform(&request.query);
    let criteria = block_criteria(&request.query)?;
    let estimate = match criteria.as_ref().is_some_and(|c| !storable(c)) {
        true => transform && ctx.config.estimate_total_count,
        false => !transform || ctx.config.estimate_total_count,
    };
    block_listing(&ctx, &request, criteria.filter(storable), estimate).await
}

/// `GET /blocks/missed`: the missed slots matching the criteria, over the lookback window.
pub async fn missed(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let criteria = missed_criteria(&ctx, &request.query)?;
    missed_listing(&ctx, &request, criteria).await
}

/// `GET /blocks/first`: the genesis block, as `GET /blocks/{id}` shows a block.
pub async fn first(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let tip = ctx.state().height();
    block(&ctx, &request, BlockSelector::Height(1), tip).await
}

/// The values a validated query gives for `name`: a list as it is, one value as a list of one.
fn values<'a>(query: &'a Value, name: &str) -> Option<&'a [Value]> {
    query.get(name).map(|value| match value {
        Value::Array(values) => values.as_slice(),
        one => std::slice::from_ref(one),
    })
}

/// The block criteria of the validated query; `None` when a key's list of values is empty, which
/// matches nothing.
pub(crate) fn block_criteria(query: &Value) -> Result<Option<BlockCriteria>, ApiError> {
    let mut criteria = BlockCriteria::default();
    for name in BLOCK_KEYS {
        let Some(values) = values(query, name) else {
            continue;
        };
        if values.is_empty() {
            return Ok(None);
        }
        match name {
            "id" => criteria.id = texts(name, values)?,
            "version" => criteria.version = integers(name, values)?,
            "timestamp" => criteria.timestamp = numerics(name, values)?,
            "previousBlock" => criteria.previous_block = texts(name, values)?,
            "height" => criteria.height = numerics(name, values)?,
            "numberOfTransactions" => criteria.number_of_transactions = numerics(name, values)?,
            "totalAmount" => criteria.total_amount = numerics(name, values)?,
            "totalFee" => criteria.total_fee = numerics(name, values)?,
            "burnedFee" => criteria.burned_fee = numerics(name, values)?,
            "reward" => criteria.reward = numerics(name, values)?,
            "payloadLength" => criteria.payload_length = numerics(name, values)?,
            "payloadHash" => criteria.payload_hash = texts(name, values)?,
            "generatorPublicKey" => criteria.generator_public_key = texts(name, values)?,
            "username" => criteria.username = texts(name, values)?,
            _ => criteria.block_signature = texts(name, values)?,
        }
    }
    Ok(Some(criteria))
}

/// The largest values of PostgreSQL's `integer` and `smallint`, the types of the reference's block
/// and missed-slot columns that are not `bigint` or text.
const INTEGER: u64 = 2_147_483_647;
const SMALLINT: u64 = 32_767;

/// Whether every value of `numerics` fits a column whose largest value is `max`. The reference's
/// store refuses a value beyond its column's type, and the listing that fails answers an empty
/// page.
fn fits(numerics: &[Numeric], max: u64) -> bool {
    numerics.iter().all(|numeric| match *numeric {
        Numeric::Equal(value) => value <= max,
        Numeric::Range { from, to } => [from, to].into_iter().flatten().all(|end| end <= max),
    })
}

/// Whether the reference's store takes every value of the block criteria `criteria`.
pub(crate) fn storable(criteria: &BlockCriteria) -> bool {
    criteria.version.iter().all(|version| *version <= SMALLINT)
        && [
            &criteria.timestamp,
            &criteria.height,
            &criteria.number_of_transactions,
            &criteria.payload_length,
        ]
        .into_iter()
        .all(|numerics| fits(numerics, INTEGER))
}

/// The keys of the block criteria.
const BLOCK_KEYS: [&str; 15] = [
    "id",
    "version",
    "timestamp",
    "previousBlock",
    "height",
    "numberOfTransactions",
    "totalAmount",
    "totalFee",
    "burnedFee",
    "reward",
    "payloadLength",
    "payloadHash",
    "generatorPublicKey",
    "username",
    "blockSignature",
];

/// The page of blocks that `criteria` (`None`: nothing) match, in the order and page of the
/// request, as the reference's blocks or, without `transform`, its raw models. `estimate` is what
/// the answer reports as `totalCountIsEstimate`.
pub(crate) async fn block_listing(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    criteria: Option<BlockCriteria>,
    estimate: bool,
) -> Result<ApiReply, ApiError> {
    let pagination = Pagination::of(&request.query);
    let page = Page {
        offset: pagination.offset,
        limit: pagination.limit,
    };
    let (order, transform) = (order(&request.query), transform(&request.query));
    let count = if estimate {
        CountFlag::Estimate
    } else {
        CountFlag::Exact
    };
    let (rows, total) = match criteria {
        Some(criteria) => {
            let filter = BlockFilter::of(criteria);
            let listing = read(ctx, move |reader| {
                reader.blocks(&filter, &order, page, transform, count)
            })
            .await?;
            (listing.rows, listing.total_count)
        }
        None => (Vec::new(), 0),
    };
    let (tip, milestones) = (ctx.state().height(), &ctx.node.crypto.milestones);
    let data = rows
        .iter()
        .map(|listed| match transform {
            true => {
                let transferred = listed.transferred.unwrap_or(0);
                resources::block(&listed.row, transferred, tip, milestones)
            }
            false => raw(&listed.row),
        })
        .collect();
    Ok(ApiReply::ok(envelope(
        request, pagination, total, estimate, data,
    )))
}

/// The missed-slot criteria of the validated query, narrowed to the lookback window as the
/// reference's listing narrows them; `None` when a key's list of values is empty. The block keys a
/// missed slot has no column for match nothing: the reference's listing fails on them and answers
/// an empty page. So does a value beyond its column's type.
pub(crate) fn missed_criteria(
    ctx: &ApiContext,
    query: &Value,
) -> Result<Option<MissedBlockCriteria>, ApiError> {
    narrowed(query, window_start(ctx))
}

/// The missed-slot criteria of `query` for a window that starts at `earliest`: no timestamp starts
/// at `earliest` (at 0 before the epoch); one exact timestamp before it becomes 0; one range that
/// starts before it, or has no start, starts at it; a list is kept. A window that starts within a
/// second (`Window::whole` false) is a start the reference's store refuses as an integer, so a
/// criteria that starts at it matches nothing.
fn narrowed(query: &Value, window: Window) -> Result<Option<MissedBlockCriteria>, ApiError> {
    let Window {
        start: earliest,
        whole,
    } = window;
    let no_column = BLOCK_KEYS.iter().any(|name| {
        !["timestamp", "height", "username"].contains(name) && query.get(name).is_some()
    });
    let empty = ["timestamp", "height", "username"]
        .iter()
        .any(|name| values(query, name).is_some_and(<[Value]>::is_empty));
    if no_column || empty {
        return Ok(None);
    }
    let one = |name: &str| values(query, name).unwrap_or_default();
    let start = u64::try_from(earliest).unwrap_or(0);
    let before = |value: u64| i128::from(value) < i128::from(earliest);
    let mut timestamp = numerics("timestamp", one("timestamp"))?;
    let starts = match (query.get("timestamp"), timestamp.as_mut_slice()) {
        (None, _) => {
            timestamp.push(Numeric::Range {
                from: Some(start),
                to: None,
            });
            true
        }
        (Some(Value::Array(_)), _) => false,
        (_, [Numeric::Equal(value)]) if before(*value) => {
            *value = 0;
            false
        }
        (_, [Numeric::Range { from, .. }]) if from.is_none_or(before) => {
            *from = Some(start);
            true
        }
        _ => false,
    };
    let height = numerics("height", one("height"))?;
    if (starts && !whole) || !fits(&timestamp, INTEGER) || !fits(&height, INTEGER) {
        return Ok(None);
    }
    Ok(Some(MissedBlockCriteria {
        timestamp,
        height,
        username: texts("username", one("username"))?,
    }))
}

/// The start of the lookback window, in whole seconds since the network epoch: the reference's
/// start, rounded up, and whether it is a whole second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    start: i64,
    whole: bool,
}

/// The first timestamp of the lookback window: the UTC midnight of the tip's date, less the
/// lookback, in seconds since the network epoch (a fractional start counts from the next whole
/// second).
fn window_start(ctx: &ApiContext) -> Window {
    let epoch = ctx.node.crypto.milestones.at(1).epoch();
    let epoch_ms = slots::epoch_unix_ms(epoch).unwrap_or(0);
    let tip = ctx.state().last_block.header().timestamp;
    window_from(tip, epoch_ms, ctx.config.missed_blocks_lookback)
}

/// [`window_start`] for a tip at `tip` seconds after an epoch of `epoch_ms` (Unix milliseconds).
fn window_from(tip: u32, epoch_ms: i64, lookback: i64) -> Window {
    const DAY: i64 = 86_400;
    let unix = (i64::from(tip) * 1000 + epoch_ms).div_euclid(1000);
    let since_epoch = unix.div_euclid(DAY) * DAY * 1000 - epoch_ms;
    let whole = since_epoch.rem_euclid(1000) == 0;
    Window {
        start: since_epoch.div_euclid(1000) + i64::from(!whole) - lookback,
        whole,
    }
}

/// The page of missed slots that `criteria` (`None`: nothing) match, in the order and page of the
/// request.
pub(crate) async fn missed_listing(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    criteria: Option<MissedBlockCriteria>,
) -> Result<ApiReply, ApiError> {
    let pagination = Pagination::of(&request.query);
    let page = Page {
        offset: pagination.offset,
        limit: pagination.limit,
    };
    let order = order(&request.query);
    let (rows, total) = match criteria {
        Some(criteria) => {
            let filter = MissedBlockFilter::of(criteria);
            let listing = read(ctx, move |reader| {
                reader.missed_blocks(&filter, &order, page)
            })
            .await?;
            (listing.rows, listing.total_count)
        }
        None => (Vec::new(), 0),
    };
    let milestones = &ctx.node.crypto.milestones;
    let data = rows
        .iter()
        .map(|row| resources::missed_block::missed_block(row, milestones))
        .collect();
    let estimate = ctx.config.estimate_total_count;
    Ok(ApiReply::ok(envelope(
        request, pagination, total, estimate, data,
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn at(start: i64) -> Window {
        Window { start, whole: true }
    }

    fn timestamps(query: &Value, earliest: i64) -> Vec<Numeric> {
        narrowed(query, at(earliest)).unwrap().unwrap().timestamp
    }

    fn starts(tip: u32, epoch_ms: i64, lookback: i64) -> i64 {
        window_from(tip, epoch_ms, lookback).start
    }

    fn range(from: u64) -> Numeric {
        Numeric::Range {
            from: Some(from),
            to: None,
        }
    }

    #[test]
    fn the_window_starts_at_the_midnight_of_the_tips_day_less_the_lookback() {
        // An epoch at a midnight: the tip 90 000 seconds in is on the second day.
        let midnight = 1_700_000_000_000 - 1_700_000_000_000 % 86_400_000;
        assert_eq!(starts(90_000, midnight, 1_000), 86_400 - 1_000);
        assert_eq!(starts(0, midnight, 0), 0);
        // An epoch that is not at a midnight: the start is in the epoch's seconds, before it.
        assert_eq!(starts(0, midnight + 3_600_000, 0), -3_600);
        // A fraction of a second rounds up.
        let fraction = window_from(0, midnight + 1_500, 0);
        assert_eq!(
            fraction,
            Window {
                start: -1,
                whole: false
            }
        );
        assert!(window_from(0, midnight + 3_600_000, 0).whole);
        assert_eq!(starts(86_400, midnight, 2_592_000), 86_400 - 2_592_000);
    }

    #[test]
    fn a_missed_slot_query_without_a_timestamp_starts_at_the_window() {
        assert_eq!(timestamps(&json!({}), 100), [range(100)]);
        assert_eq!(timestamps(&json!({"height": 3}), 100), [range(100)]);
        // Before the epoch the window starts at 0.
        assert_eq!(timestamps(&json!({}), -5), [range(0)]);
    }

    #[test]
    fn a_timestamp_before_the_window_is_narrowed() {
        assert_eq!(
            timestamps(&json!({"timestamp": 50}), 100),
            [Numeric::Equal(0)]
        );
        assert_eq!(
            timestamps(&json!({"timestamp": 150}), 100),
            [Numeric::Equal(150)]
        );
        let low = json!({"timestamp": {"from": 20, "to": 500}});
        let narrowed = Numeric::Range {
            from: Some(100),
            to: Some(500),
        };
        assert_eq!(timestamps(&low, 100), [narrowed]);
        let open = json!({"timestamp": {"to": 500}});
        assert_eq!(
            timestamps(&open, 100),
            [Numeric::Range {
                from: Some(100),
                to: Some(500)
            }]
        );
        let kept = json!({"timestamp": {"from": 300}});
        assert_eq!(timestamps(&kept, 100), [range(300)]);
    }

    #[test]
    fn a_list_of_timestamps_is_kept() {
        let got = timestamps(&json!({"timestamp": [10, 20]}), 100);
        assert_eq!(got, [Numeric::Equal(10), Numeric::Equal(20)]);
    }

    #[test]
    fn a_block_key_without_a_column_matches_nothing() {
        for key in ["reward", "id", "generatorPublicKey", "totalFee"] {
            assert_eq!(narrowed(&json!({key: 1}), at(0)).unwrap(), None, "{key}");
        }
        assert_eq!(narrowed(&json!({"height": []}), at(0)).unwrap(), None);
    }

    #[test]
    fn a_value_beyond_its_columns_type_matches_nothing() {
        let beyond = INTEGER + 1;
        for query in [
            json!({"timestamp": beyond}),
            json!({"timestamp": {"from": 300, "to": beyond}}),
            json!({"timestamp": [300, beyond]}),
            json!({"height": {"to": beyond}}),
        ] {
            assert_eq!(narrowed(&query, at(100)).unwrap(), None, "{query}");
        }
        assert!(
            narrowed(&json!({"height": INTEGER}), at(100))
                .unwrap()
                .is_some()
        );
        let block = |query: Value| block_criteria(&query).unwrap().unwrap();
        assert!(storable(&block(
            json!({"version": 32_767, "height": INTEGER})
        )));
        assert!(storable(&block(
            json!({"totalAmount": u64::MAX, "reward": beyond})
        )));
        for query in [
            json!({"version": 32_768}),
            json!({"timestamp": beyond}),
            json!({"height": {"from": 1, "to": beyond}}),
            json!({"numberOfTransactions": [0, beyond]}),
            json!({"payloadLength": {"from": beyond}}),
        ] {
            assert!(!storable(&block(query.clone())), "{query}");
        }
    }

    #[test]
    fn a_window_within_a_second_is_a_start_the_store_refuses() {
        let within = Window {
            start: 100,
            whole: false,
        };
        // A criteria that starts at the window's start fails the reference's listing.
        assert_eq!(narrowed(&json!({}), within).unwrap(), None);
        let open = json!({"timestamp": {"to": 500}});
        assert_eq!(narrowed(&open, within).unwrap(), None);
        // One that keeps its own start, or its exact value, does not.
        let kept = narrowed(&json!({"timestamp": {"from": 300}}), within).unwrap();
        assert_eq!(kept.unwrap().timestamp, [range(300)]);
        let early = narrowed(&json!({"timestamp": 50}), within).unwrap();
        assert_eq!(early.unwrap().timestamp, [Numeric::Equal(0)]);
        let list = narrowed(&json!({"timestamp": [10, 20]}), within).unwrap();
        assert!(list.is_some());
    }

    #[test]
    fn the_block_criteria_take_each_key_and_an_empty_list_matches_nothing() {
        let got = block_criteria(&json!({
            "height": {"from": 2, "to": 9},
            "username": "genesis_1",
            "version": [0, 1],
            "orderBy": ["height:desc"],
        }))
        .unwrap()
        .unwrap();
        assert_eq!(got.username, ["genesis_1"]);
        assert_eq!(got.version.len(), 2);
        assert_eq!(
            got.height,
            [Numeric::Range {
                from: Some(2),
                to: Some(9)
            }]
        );
        assert!(block_criteria(&json!({"id": []})).unwrap().is_none());
        assert_eq!(
            block_criteria(&json!({})).unwrap().unwrap(),
            BlockCriteria::default()
        );
    }
}
