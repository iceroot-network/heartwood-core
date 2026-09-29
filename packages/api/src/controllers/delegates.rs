//! The delegate routes over the state: `GET /delegates` (the delegate search over the username
//! index), `GET /delegates/{id}` (one delegate's resource) and `GET /delegates/{id}/voters` (the
//! wallets that vote for the delegate, over the public-key index).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::errors::ApiError;

/// `GET /delegates`: the delegates matching the criteria, sorted and paginated.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn index(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /delegates/{id}`: one delegate, by username, address or public key.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn show(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}

/// `GET /delegates/{id}/voters`: the wallets voting for a delegate.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn voters(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
