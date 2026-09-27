//! The pool routes and the static transaction routes: `POST /transactions` (at most the request
//! limit of entries, to the API channel's processor; `errors` absent when empty; 422 on a failed
//! validation), `GET /transactions/unconfirmed` (the pool from the highest priority, paginated,
//! `burnedFee` left out), `GET /transactions/unconfirmed/{id}` (raw by default, transformed for any
//! non-empty `transform`), `GET /transactions/types` (the six kept types) and
//! `GET /transactions/fees` (the static fees of the tip's milestone).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::TransactionsStore,
            handler: |ctx, request| Box::pin(store(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsUnconfirmed,
            handler: |ctx, request| Box::pin(unconfirmed(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsShowUnconfirmed,
            handler: |ctx, request| Box::pin(show_unconfirmed(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsTypes,
            handler: |ctx, request| Box::pin(types(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsFees,
            handler: |ctx, request| Box::pin(fees(ctx, request)),
        },
    ]
}

/// Submit transactions to the pool.
pub async fn store(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The pool's transactions.
pub async fn unconfirmed(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// A pool transaction by id.
pub async fn show_unconfirmed(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The transaction types.
pub async fn types(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The static fees.
pub async fn fees(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
