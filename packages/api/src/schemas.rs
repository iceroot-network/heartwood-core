//! The query grammar and the routes' validation, as the reference's server reads them.
//!
//! [`parse_query`] reads a query string as the reference's `qs` parse, then its comma-array and
//! dot-nesting plugins: `a=1,2` is `["1", "2"]` (any value, `memo` included), `a.b=1` is
//! `{"a": {"b": "1"}}`. [`validate_query`], [`validate_params`] and [`validate_payload`] apply each
//! route's schema: the accepted keys, ranges and defaults, values coerced as the reference's
//! schemas coerce them; a failure is 422 with the schema's message, and an unknown query key is 422
//! on the routes whose schema lists their keys.

use serde_json::{Map, Value};

use crate::context::RouteId;
use crate::errors::ApiError;

/// The query string `raw` (without the `?`), parsed.
pub fn parse_query(raw: &str) -> Value {
    let _ = raw;
    todo!()
}

/// The query of `route`, validated: defaults added and values coerced.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let _ = (route, query);
    todo!()
}

/// Validate the path parameters of `route`.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    let _ = (route, params);
    todo!()
}

/// The payload of `route` (`POST /transactions`), validated.
pub fn validate_payload(route: RouteId, payload: &Value) -> Result<Value, ApiError> {
    let _ = (route, payload);
    todo!()
}
