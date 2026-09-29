//! `functional-red-black-tree` 1.0.1's insert and remove, as the reference's `getTop` uses them:
//! with their search paths and rebalancing, the pre-check against the end key, and the eviction of
//! the end, so that the pairs the tree compares, and so the comparisons that throw, are the
//! reference's.

use std::cmp::Ordering;

use crate::errors::ApiError;

/// The first `count` of `items` in the order of `compare`, which receives each item with its
/// iteration index; the first error of `compare` is the answer.
pub fn top<T>(
    count: usize,
    items: impl Iterator<Item = T>,
    compare: impl FnMut(&(usize, T), &(usize, T)) -> Result<Ordering, ApiError>,
) -> Result<Vec<T>, ApiError> {
    let _ = (count, items, compare);
    todo!("the red-black tree's top")
}
