//! The chain routes: `GET /blockchain` (the tip, the burned totals and the supply) and
//! `GET /blockchain/search/{id}` (blocks, transactions and wallets whose id or name starts with the
//! lower-cased input).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::errors::ApiError;

/// `GET /blockchain`: the tip, the burned totals and the supply.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /blockchain/search/{id}`: the blocks, transactions and wallets the input starts.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn search(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
