//! The server's plugins, around every route: the rate limit (100 points per 60 s per address,
//! charged before validation, its whitelist and blacklist; 429 without `Retry-After`), the address
//! whitelist (403), `X-Block-Height` (errors included), `connection: close`, CORS in hapi's shape,
//! trailing slashes stripped, 404 (not 405) for another method, the 1 MiB body limit, the
//! semaphore (bounded concurrency of the store's and the memory's routes), the query grammar and
//! the routes' validation ([`crate::schemas`]), and Boom's error bodies.

use std::sync::Arc;

use axum::Router;

use crate::context::{ApiContext, Route};

/// The router of `routes` under the base path, wrapped in the plugins.
pub fn router(routes: Vec<Route>, context: Arc<ApiContext>) -> Router {
    let _ = (routes, context);
    todo!()
}
