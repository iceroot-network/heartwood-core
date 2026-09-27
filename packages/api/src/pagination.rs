//! The list envelope: `meta` (`totalCountIsEstimate`, `count`, `pageCount`, `totalCount`, `next`,
//! `previous`, `self`, `first`, `last`, in that order) and `data`, with the reference's link
//! strings (the path, the query with its `page` and `limit`, `orderBy` kept). `offset=0` is ignored,
//! as the reference ignores it; another offset replaces the page.

use serde_json::Value;

use crate::context::ApiRequest;

/// A page of a list route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// The matches skipped.
    pub offset: u64,
    /// The most matches returned.
    pub limit: u64,
}

impl Pagination {
    /// The page that the validated query `query` asks for (`page`, `limit`, `offset`).
    pub fn of(query: &Value) -> Pagination {
        let _ = query;
        todo!()
    }
}

/// The list envelope of `data`, the page `pagination` of `total_count` matches of `request`.
pub fn envelope(
    request: &ApiRequest,
    pagination: Pagination,
    total_count: u64,
    total_count_is_estimate: bool,
    data: Vec<Value>,
) -> Value {
    let _ = (
        request,
        pagination,
        total_count,
        total_count_is_estimate,
        data,
    );
    todo!()
}
