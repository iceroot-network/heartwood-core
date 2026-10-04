//! The round route: `GET /rounds/{id}/delegates` (the stored round's delegates and balances).

use std::sync::Arc;

use heartwood_database::filters::js_number;

use crate::context::{ApiContext, ApiReply, ApiRequest};
use crate::controllers::transactions::read;
use crate::errors::ApiError;
use crate::resources::round::round_delegate;

/// `GET /rounds/{id}/delegates`: a round's delegates, in their stored order (by balance from the
/// highest, then by public key); 404 "Round not found" when the store holds none.
pub async fn delegates(ctx: Arc<ApiContext>, request: ApiRequest) -> Result<ApiReply, ApiError> {
    let not_found = || ApiError::not_found("Round not found");
    // The parameter is a whole number from 1 (a safe one), so the conversion is exact.
    let round = request
        .param("id")
        .map(js_number)
        .filter(|round| *round >= 1.0);
    let round = round.ok_or_else(not_found)?;
    let rows = read(&ctx, move |reader| reader.round(round as u64)).await?;
    if rows.is_empty() {
        return Err(not_found());
    }
    Ok(ApiReply::data(rows.iter().map(round_delegate).collect()))
}
