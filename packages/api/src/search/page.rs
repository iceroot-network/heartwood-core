//! The reference's pagination service as it runs: `getPage` hands the filtered resources to
//! `getTop(sorting, offset + limit)`, whose red-black tree ([`super::rb_tree`]) keeps the first
//! ones in the sorting's order. The comparator walks the sorting chain (values first, then `null`,
//! then absent values, in both directions; strings in UTF-16 order), then the iteration index; a
//! comparison of mixed types or of two plain objects that the tree makes is the reference's 500.

use serde_json::Value;

use super::{ResultsPage, Sorting};
use crate::errors::ApiError;
use crate::pagination::Pagination;

/// The page `pagination` of `items` in the order `sorting`, with every item's count.
pub fn page(
    pagination: Pagination,
    sorting: &Sorting,
    items: impl Iterator<Item = Value>,
) -> Result<ResultsPage, ApiError> {
    let _ = (pagination, sorting, items);
    todo!("the pagination service's getPage")
}
