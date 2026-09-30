//! The in-memory listings' search, as the reference's kernel search services run it: the
//! standard criteria ([`criteria`]), the pagination service's `getPage` through `getTop`'s
//! red-black tree ([`mod@page`], [`rb_tree`]), and the wallet part of the chain search with its
//! collation and Node 18's sort ([`mod@wallets_like`], [`collate`], [`mod@v8_sort`]).
//!
//! The module reads only the published state and the resources built from it: it never opens a
//! store connection, so a listing over the state never waits on the store (`tools/check-layers.sh`
//! holds that).

pub mod collate;
pub(crate) mod convert;
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
/// property. A resource matches a list of them when it matches every one.
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

/// The key of a BigNumber value in a resource that the criteria and the sorting read: `{"$bigNumber":
/// "<decimal>"}`. A resource is built with these and written out with [`plain`].
pub const BIG_NUMBER: &str = "$bigNumber";

/// The key of a Semver value in a resource: `{"$semver": "<version>"}`.
pub const SEMVER: &str = "$semver";

fn tagged(key: &str, text: String) -> Value {
    let mut map = Map::new();
    map.insert(key.to_owned(), Value::String(text));
    Value::Object(map)
}

/// A BigNumber of `decimal`, as a resource holds it.
pub fn big_number(decimal: impl ToString) -> Value {
    tagged(BIG_NUMBER, decimal.to_string())
}

/// A Semver of `version`, as a resource holds it.
pub fn semver(version: impl ToString) -> Value {
    tagged(SEMVER, version.to_string())
}

/// The kind and the text of a BigNumber or Semver value.
pub fn marker(map: &Map<String, Value>) -> Option<(&'static str, &str)> {
    let (key, value) = map.iter().next().filter(|_| map.len() == 1)?;
    let kind = [BIG_NUMBER, SEMVER].into_iter().find(|kind| kind == key)?;
    Some((kind, value.as_str()?))
}

/// `value` with each BigNumber and Semver as its text, which is how the reference writes them.
pub fn plain(value: Value) -> Value {
    match value {
        Value::Object(map) => match marker(&map) {
            Some((_, text)) => Value::String(text.to_owned()),
            None => Value::Object(
                map.into_iter()
                    .map(|(key, member)| (key, plain(member)))
                    .collect(),
            ),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(plain).collect()),
        other => other,
    }
}
