//! The reference's pagination service as it runs: `getPage` hands the filtered resources to
//! `getTop(sorting, offset + limit)`, whose red-black tree ([`super::rb_tree`]) keeps the first
//! ones in the sorting's order. The comparator walks the sorting chain (values first, then `null`,
//! then absent values, in both directions; strings in UTF-16 order), then the iteration index; a
//! comparison of mixed types or of two plain objects that the tree makes is the reference's 500.

use std::cmp::Ordering;

use heartwood_crypto::utils::js;
use heartwood_p2p::utils::npm_semver;
use serde_json::{Map, Value};

use super::rb_tree::top;
use super::{BIG_NUMBER, Direction, ResultsPage, SEMVER, Sorting};
use crate::errors::ApiError;
use crate::pagination::Pagination;

/// The path segments the reference's `get` refuses: a path with one reads the object itself.
const DISALLOWED: [&str; 3] = ["__proto__", "prototype", "constructor"];

/// The page `pagination` of `items` in the order `sorting`, with every item's count.
pub fn page(
    pagination: Pagination,
    sorting: &Sorting,
    items: impl Iterator<Item = Value>,
) -> Result<ResultsPage, ApiError> {
    let all: Vec<Value> = items.collect();
    let total_count = all.len() as u64;
    let index = |count: u64| usize::try_from(count).unwrap_or(usize::MAX);
    let offset = index(pagination.offset);
    let end = index(pagination.offset.saturating_add(pagination.limit));
    let kept = if sorting.0.is_empty() {
        all
    } else {
        top(end, all.into_iter(), |a, b| compare(a, b, sorting))?
    };
    let results = kept
        .into_iter()
        .skip(offset)
        .take(end.saturating_sub(offset));
    Ok(ResultsPage {
        results: results.collect(),
        total_count,
        total_count_is_estimate: false,
    })
}

/// What the reference's `get` gives, by the type the comparator tells apart.
#[derive(Clone)]
enum Val<'a> {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    /// A string's UTF-16 code units (indexing a string gives one, which may be half a pair).
    Str(Vec<u16>),
    /// A native bigint, as decimal text.
    Big(&'a str),
    BigNumber(&'a str),
    Semver(&'a str),
    Object(&'a Map<String, Value>),
    Array(&'a [Value]),
}

impl<'a> Val<'a> {
    fn of(value: &'a Value) -> Self {
        match value {
            Value::Null => Val::Null,
            Value::Bool(flag) => Val::Bool(*flag),
            Value::Number(number) => Val::Number(js::number_value(number)),
            Value::String(text) => Val::Str(text.encode_utf16().collect()),
            Value::Array(items) => Val::Array(items),
            Value::Object(map) => match map.iter().next().filter(|_| map.len() == 1) {
                Some((key, Value::String(text))) if key == BIG_NUMBER => Val::BigNumber(text),
                Some((key, Value::String(text))) if key == SEMVER => Val::Semver(text),
                _ => Val::Object(map),
            },
        }
    }

    /// The own enumerable property `segment` of the value, if it has one.
    fn member(&self, segment: &str) -> Option<Val<'a>> {
        let index = || js::is_array_index(segment).then(|| segment.parse::<usize>().ok())?;
        match self {
            Val::Object(map) => map.get(segment).map(Val::of),
            Val::Array(items) => items.get(index()?).map(Val::of),
            Val::Str(units) => units.get(index()?).map(|&unit| Val::Str(vec![unit])),
            Val::BigNumber(text) if segment == "value" => Some(Val::Big(text)),
            Val::Semver(text) if segment == "value" => {
                Some(Val::Str(text.encode_utf16().collect()))
            }
            _ => None,
        }
    }
}

/// The reference's `get(item, path)`: an item that is not an object, or a path that leaves it,
/// reads as absent; a segment of `null` in the middle too. Reading a property of `null` throws.
fn get<'a>(item: &'a Value, path: &str) -> Result<Val<'a>, ApiError> {
    let parts: Vec<&str> = path.split('.').collect();
    let segments: &[&str] = if parts.iter().any(|part| DISALLOWED.contains(part)) {
        &[]
    } else {
        &parts
    };
    let mut current = Val::of(item);
    if matches!(current, Val::Bool(_) | Val::Number(_) | Val::Str(_)) {
        return Ok(Val::Undefined);
    }
    for (position, segment) in segments.iter().enumerate() {
        if matches!(current, Val::Null) {
            return Err(ApiError::internal());
        }
        let Some(next) = current.member(segment) else {
            return Ok(Val::Undefined);
        };
        if matches!(next, Val::Null) && position + 1 < segments.len() {
            return Ok(Val::Undefined);
        }
        current = next;
    }
    Ok(current)
}

/// The reference's `compare`: down the sorting, then by iteration index.
fn compare(
    a: &(usize, Value),
    b: &(usize, Value),
    sorting: &Sorting,
) -> Result<Ordering, ApiError> {
    for (property, direction) in &sorting.0 {
        let (mut x, mut y) = (get(&a.1, property)?, get(&b.1, property)?);
        // Absent values come after `null`, which comes after every value, in both directions.
        let rank = |value: &Val<'_>| match value {
            Val::Undefined => 2,
            Val::Null => 1,
            _ => 0,
        };
        let by_rank = rank(&x).cmp(&rank(&y));
        if by_rank != Ordering::Equal {
            return Ok(by_rank);
        }
        if rank(&x) > 0 {
            continue;
        }
        if *direction == Direction::Desc {
            (x, y) = (y, x);
        }
        let order = match (x, y) {
            (Val::Bool(x), Val::Bool(y)) => x.cmp(&y),
            (Val::Number(x), Val::Number(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Val::Str(x), Val::Str(y)) => x.cmp(&y),
            (Val::Big(x), Val::Big(y)) | (Val::BigNumber(x), Val::BigNumber(y)) => big(x, y)?,
            (Val::Semver(x), Val::Semver(y)) => semver(x, y)?,
            _ => return Err(ApiError::internal()),
        };
        if order != Ordering::Equal {
            return Ok(order);
        }
    }
    Ok(a.0.cmp(&b.0))
}

/// The sign and the digits, without leading zeros, of an integer in decimal text.
fn integer(text: &str) -> Result<(bool, &str), ApiError> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError::internal());
    }
    let digits = digits.trim_start_matches('0');
    Ok((text.starts_with('-') && !digits.is_empty(), digits))
}

/// The order of two integers in decimal text; text that is no integer is not a BigNumber.
fn big(a: &str, b: &str) -> Result<Ordering, ApiError> {
    let ((a_negative, a), (b_negative, b)) = (integer(a)?, integer(b)?);
    let magnitude = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
    let signed = if a_negative {
        magnitude.reverse()
    } else {
        magnitude
    };
    Ok(b_negative.cmp(&a_negative).then(signed))
}

/// `Semver.isLessThan` and `isGreaterThan`: the first version as it is, the other one cleaned
/// (`semver.clean`); a version that is not valid throws.
fn semver(a: &str, b: &str) -> Result<Ordering, ApiError> {
    let space = |c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}';
    npm_semver::valid(b.trim_matches(space).trim_start_matches(['=', 'v']))
        .and_then(|b| npm_semver::compare(a, &b))
        .ok_or_else(ApiError::internal)
}
