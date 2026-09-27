//! The node routes: `GET /node/status` (`synced`, `now`, `blocksCount`, `timestamp`),
//! `GET /node/syncing` (`syncing`, `blocks`, `height`, `id`), `GET /node/configuration` (the
//! network's values, the tip's milestone as `constants`, the pool's options, `ports: {}` and the
//! node's version) and `GET /node/configuration/crypto` (the loaded crypto files, the milestones
//! merged cumulatively, `exceptions: {}`).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest, Route, RouteId};
use crate::errors::ApiError;

/// The routes of this controller.
pub fn routes() -> Vec<Route> {
    vec![
        Route {
            id: RouteId::NodeStatus,
            handler: |ctx, request| Box::pin(status(ctx, request)),
        },
        Route {
            id: RouteId::NodeSyncing,
            handler: |ctx, request| Box::pin(syncing(ctx, request)),
        },
        Route {
            id: RouteId::NodeConfiguration,
            handler: |ctx, request| Box::pin(configuration(ctx, request)),
        },
        Route {
            id: RouteId::NodeConfigurationCrypto,
            handler: |ctx, request| Box::pin(configuration_crypto(ctx, request)),
        },
    ]
}

/// The node's status.
pub async fn status(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The node's sync.
pub async fn syncing(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The node's configuration.
pub async fn configuration(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}

/// The crypto configuration.
pub async fn configuration_crypto(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    todo!()
}
