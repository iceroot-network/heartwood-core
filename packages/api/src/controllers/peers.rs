//! The peer routes: `GET /peers` (the `version` filter as a semver range, a bad range 404 "Invalid
//! version range provided"; the `ip` filter; latency order; pagination) and `GET /peers/{ip}` (404
//! "Peer not found").

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::PeersIndex,
            handler: |ctx, request| Box::pin(index(ctx, request)),
        },
        Route {
            id: RouteId::PeersShow,
            handler: |ctx, request| Box::pin(show(ctx, request)),
        },
    ]
}

/// The peers.
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// A peer by address.
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
