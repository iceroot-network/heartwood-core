//! Node 18's `Array.prototype.sort`: V8's TimSort (binary insertion below 64 elements; runs,
//! merges and galloping above), so that a comparator that is not a total order gives the
//! reference's order.

use std::cmp::Ordering;

/// Sort `items` in place with `compare` as V8 sorts an array.
pub fn v8_sort<T>(items: &mut [T], compare: impl FnMut(&T, &T) -> Ordering) {
    let _ = (items, compare);
    todo!("V8's sort")
}
