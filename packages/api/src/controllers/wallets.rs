//! The wallet routes: `GET /wallets/{id}` (by address, public key or username; a valid unknown
//! address is a zero wallet; another unknown 34-character id is 422 "Wallet not valid"; anything
//! else unknown 404; the basic wallet's attributes, the delegate's version from the online record)
//! and `GET /wallets/{id}/transactions` (the `address` criteria of the wallet, through the history
//! queries).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::WalletsShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
        Route {
            id: RouteId::WalletsTransactions,
            handler: |ctx, request| Box::pin(transactions(ctx, request)),
        },
    ]
}

/// A wallet.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// A wallet's transactions.
pub async fn transactions(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
