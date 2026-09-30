//! A delegate's history in the store: `GET /delegates/{id}/blocks` (its blocks) and
//! `GET /delegates/{id}/blocks/missed` (its missed slots).
//!
//! `{id}` is found as a wallet and must be a delegate's: anything else is 404 "Delegate not
//! found". The delegate's name replaces any `username` criterion.

use std::sync::Arc;

use heartwood_database::BlockCriteria;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::controllers::blocks::{
    block_criteria, block_listing, missed_criteria, missed_listing, storable,
};
use crate::controllers::wallets;
use crate::errors::ApiError;

/// `GET /delegates/{id}/blocks`: a delegate's blocks; the query's `transform` chooses the
/// resource, and the count flag is the configured one either way (a value beyond its column's type
/// fails the reference's listing, which answers an empty page with the same flag).
pub async fn blocks(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let username = delegate_name(&ctx, &request)?;
    let criteria = block_criteria(&request.query)?.filter(storable);
    let criteria = criteria.map(|criteria| BlockCriteria {
        username: vec![username],
        ..criteria
    });
    block_listing(&ctx, &request, criteria, ctx.config.estimate_total_count).await
}

/// `GET /delegates/{id}/blocks/missed`: a delegate's missed slots. The block keys other than
/// `timestamp` and `height` match nothing, as on `GET /blocks/missed`.
pub async fn missed_blocks(
    ctx: Arc<ApiContext>,
    request: ApiRequest,
) -> Result<ApiReply, ApiError> {
    let username = delegate_name(&ctx, &request)?;
    let criteria = missed_criteria(&ctx, &request.query)?.map(|criteria| {
        heartwood_database::MissedBlockCriteria {
            username: vec![username],
            ..criteria
        }
    });
    missed_listing(&ctx, &request, criteria).await
}

/// The name of the delegate that `{id}` finds.
fn delegate_name(ctx: &ApiContext, request: &ApiRequest) -> Result<String, ApiError> {
    let not_found = || ApiError::not_found("Delegate not found");
    let wallet = wallets::find(&ctx.state(), request.param("id").unwrap_or_default())
        .map_err(|_| not_found())?;
    let delegate = wallet.delegate().ok_or_else(not_found)?;
    Ok(delegate.username().to_owned())
}
