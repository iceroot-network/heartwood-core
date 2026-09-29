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
use heartwood_crypto::transactions::types::Asset;
use heartwood_database::filters::js_number;
use heartwood_database::{BlockRow, BlockSelector, TransactionCriteria};
use serde_json::{Map, Value};

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::controllers::transactions::{
    Shape, criteria, decode, is_lowercase_hex, listing, read, transform,
};
use crate::errors::ApiError;
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
fn raw(row: &BlockRow) -> Value {
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

/// `GET /blocks`: the blocks matching the criteria, sorted and paginated.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /blocks/missed`: the missed slots matching the criteria.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn missed(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /blocks/first`: the genesis block.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn first(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
