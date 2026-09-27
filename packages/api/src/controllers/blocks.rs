//! The block routes: `GET /blocks/last` (the tip), `GET /blocks/{id}` (a full id or a height; a
//! height above the tip is 404 "Block not found") and `GET /blocks/{id}/transactions` (the block's
//! transactions with the transaction criteria and pagination, each without its `timestamp` and with
//! `confirmations: 0`; 404 unless the id is a full id or a height at or below the tip).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

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
    let _ = (ctx, request);
    todo!()
}

/// A block by id or height.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// A block's transactions.
pub async fn transactions(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
