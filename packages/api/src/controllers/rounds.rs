//! The round route: `GET /rounds/{id}/delegates` (the stored round's delegates and balances).

use std::sync::Arc;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::errors::ApiError;

/// `GET /rounds/{id}/delegates`: a round's delegates, in their stored order.
///
/// Not served yet: until its handler is written, the route answers what an unmatched path
/// answers, 404 "Not Found".
pub async fn delegates(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let _ = (ctx, request);
    Err(ApiError::not_found("Not Found"))
}
