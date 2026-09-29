//! A delegate's history in the store: `GET /delegates/{id}/blocks` (its blocks) and
//! `GET /delegates/{id}/blocks/missed` (its missed slots).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::errors::ApiError;

/// `GET /delegates/{id}/blocks`: a delegate's blocks.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn blocks(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /delegates/{id}/blocks/missed`: a delegate's missed slots.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn missed_blocks(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
