//! The schemas of the state's listings: the delegate criteria of `GET /delegates` and
//! `GET /delegates/{id}/voters`, the wallet criteria of `GET /wallets` and `GET /wallets/top`
//! (`attributes` an open object, as the reference's `Joi.object()`), their sortable properties,
//! and `GET /delegates/{id}`'s parameter.
//!
//! Not written yet: until its schemas are, each route here takes any query and any parameter, and
//! its handler answers 404.

use serde_json::{Map, Value};

use crate::context::RouteId;
use crate::errors::ApiError;

/// The query of `route`, validated: defaults added and values coerced, or 422.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let _ = route;
    Ok(query.clone())
}

/// Validate the path parameters of `route`.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    let _ = (route, params);
    Ok(())
}
