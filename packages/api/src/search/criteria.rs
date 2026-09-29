//! The reference's standard criteria: exact values; on a string value, a string criterion only,
//! with `%` as an unanchored in-order substring; ranges `{from, to}` on numbers, BigNumbers and
//! Semvers only; arrays as alternatives; nested objects and `attributes.*` paths; the `votingFor`
//! wildcard; a boolean matching `true`, `false`, `"true"` and `"false"`; absent and `null` values
//! never matching; and the reference's `InvalidCriteria` and `UnsupportedValue` throws.

use serde_json::Value;

use super::Criteria;
use crate::errors::ApiError;

/// Whether `resource` matches one of `criterias` (all of them when the list is empty).
///
/// An error is the reference's `InvalidCriteria` or `UnsupportedValue`, which no controller
/// catches: the route answers 500.
pub fn matches(resource: &Value, criterias: &[Criteria]) -> Result<bool, ApiError> {
    let _ = (resource, criterias);
    todo!("the standard criteria")
}
