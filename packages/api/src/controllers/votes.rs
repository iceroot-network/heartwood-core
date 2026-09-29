//! The vote routes: `GET /votes` (the vote transactions) and `GET /votes/{id}` (one of them).
//!
//! A vote is a Core Vote (type group 1, type 3) or a Solar Vote (type group 2, type 2); the
//! listings match either, whatever `typeGroup` and `type` the query gives.

use std::sync::Arc;

use heartwood_crypto::TransactionId;
use heartwood_database::{TransactionCriteria, TransactionFilter};
use serde_json::Value;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::controllers::transactions::{
    Shape, criteria, filtered, is_lowercase_hex, present, read, transform,
};
use crate::errors::ApiError;

/// `GET /votes`: the vote transactions matching the criteria, sorted and paginated.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    votes(&ctx, &request, None, Shape::Plain).await
}

/// `GET /votes/{id}`: one vote transaction; 404 "Vote not found" for another kind of
/// transaction too.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let not_found = || ApiError::not_found("Vote not found");
    // The reference matches the id as a prefix of the stored lowercase id.
    let id = request
        .param("id")
        .filter(|id| is_lowercase_hex(id))
        .and_then(|id| TransactionId::from_hex(id).ok())
        .ok_or_else(not_found)?;
    let record = read(&ctx, move |reader| reader.transaction(&id))
        .await?
        .filter(|record| matches!((record.row.type_group, record.row.kind), (1, 3) | (2, 2)))
        .ok_or_else(not_found)?;
    let tip = ctx.state().height();
    let shape = Shape::Plain;
    let data = present(&record.row, shape, transform(&request.query), tip, &ctx)?;
    Ok(ApiReply::data(data))
}

/// The vote transactions of the request's criteria, as `shape`; `sender` (an address) replaces
/// the query's `senderId`. The query's `typeGroup` and `type` are replaced by the vote kinds.
pub(crate) async fn votes(
    ctx: &Arc<ApiContext>,
    request: &ApiRequest,
    sender: Option<String>,
    shape: Shape,
) -> Result<ApiReply, ApiError> {
    let mut query = request.query.clone();
    if let Value::Object(map) = &mut query {
        let replaced = ["typeGroup", "type"]
            .into_iter()
            .chain(sender.as_deref().map(|_| "senderId"));
        for name in replaced {
            map.shift_remove(name);
        }
    }
    let filter = criteria(&query)?.map(|base| {
        let sender_id = sender.map_or_else(|| base.sender_id.clone(), |address| vec![address]);
        let vote = |type_group, kind| TransactionCriteria {
            type_group: vec![type_group],
            kind: vec![kind],
            sender_id: sender_id.clone(),
            ..base.clone()
        };
        TransactionFilter {
            groups: vec![vec![vote(1, 3), vote(2, 2)]],
        }
    });
    filtered(ctx, request, filter, shape).await
}
