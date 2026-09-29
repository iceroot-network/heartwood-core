//! The in-memory listings' search, as the reference's kernel search services run it: the
//! standard criteria ([`criteria`]), the pagination service's `getPage` through `getTop`'s
//! red-black tree ([`mod@page`], [`rb_tree`]), and the wallet part of the chain search with its
//! collation and Node 18's sort ([`mod@wallets_like`], [`collate`], [`mod@v8_sort`]).
//!
//! The module reads only the published state and the resources built from it: it never opens a
//! store connection, so a listing over the state never waits on the store (`tools/check-layers.sh`
//! holds that).

pub mod collate;
pub mod criteria;
pub mod page;
pub mod rb_tree;
pub mod v8_sort;
pub mod wallets_like;

pub use self::collate::locale_compare;
pub use self::criteria::matches;
pub use self::page::page;
pub use self::v8_sort::v8_sort;
pub use self::wallets_like::wallets_like;

use serde_json::{Map, Value};

/// A direction of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Lowest first.
    Asc,
    /// Highest first.
    Desc,
}

/// One criteria object of the validated query, in the reference's object shape: property to value,
/// where an array is an alternative of values, `{from, to}` a range, and a nested object a nested
/// property. A list of them is the reference's or-criteria: a resource matches when one matches.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Criteria(pub Map<String, Value>);

/// The sorting of a listing: the query's `orderBy` with the route's default tail, each a property
/// path (`attributes.delegate.rank`) and its direction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sorting(pub Vec<(String, Direction)>);

/// A page of a listing, as the reference's `ResultsPage`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultsPage {
    /// The page's resources, in order.
    pub results: Vec<Value>,
    /// Every matching resource's count.
    pub total_count: u64,
    /// Whether the count is an estimate; never for a listing over the state.
    pub total_count_is_estimate: bool,
}
