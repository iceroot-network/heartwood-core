//! The confirmed transaction routes: `GET /transactions` (the reference's criteria, the address
//! through the recipients side table, the implicit type group, the byte-wise memo pattern, the
//! asset containment; default order `timestamp:desc,sequence:desc`, ties broken by the unique key;
//! pagination) and `GET /transactions/{id}` (with its block; `confirmations` is the tip minus its
//! height plus one).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

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
    let _ = (ctx, request);
    todo!()
}

/// A transaction by id.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
