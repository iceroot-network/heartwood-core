//! The vote routes: `GET /votes` (the vote transactions) and `GET /votes/{id}` (one of them).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::errors::ApiError;

/// `GET /votes`: the vote transactions matching the criteria.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /votes/{id}`: one vote transaction.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
