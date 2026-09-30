//! The reference's standard criteria: exact values; on a string value, a string criterion only,
//! with `%` as an unanchored in-order substring; ranges `{from, to}` on numbers, BigNumbers and
//! Semvers only; arrays as alternatives; nested objects and `attributes.*` paths; the `votingFor`
//! wildcard; a boolean matching `true`, `false`, `"true"` and `"false"`; absent and `null` values
//! never matching; and the reference's `InvalidCriteria` and `UnsupportedValue` throws.

use std::borrow::Cow;
use std::cmp::Ordering;

use heartwood_crypto::utils::js::property_order;
use heartwood_p2p::utils::npm_semver;
use serde_json::{Map, Value};

use super::convert::{Big, is_space, number, text};
use super::{BIG_NUMBER, Criteria, Direction, Sorting, marker};
use crate::errors::ApiError;

type Test = Result<bool, ApiError>;

/// The properties an object has without holding them: reading one gives a function, which the
/// reference does not support as a value.
const INHERITED: [&str; 11] = [
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// The criteria of a validated listing query (`getQueryCriteria`): its members but the paging,
/// the order and the parsed copy.
pub fn of_query(query: &Value) -> Criteria {
    let paging = ["orderBy", "page", "limit", "offset", crate::schemas::PARSED];
    let members = query.as_object().into_iter().flatten();
    let members = members.filter(|(key, _)| !paging.contains(&key.as_str()));
    Criteria(
        members
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The sorting of a validated listing query: its `orderBy`, then the route's `tail`.
pub fn sorting_of(query: &Value, tail: &[(&str, Direction)]) -> Sorting {
    let given = query
        .get("orderBy")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let given = given.filter_map(|order| {
        let property = order.get("property")?.as_str()?.to_owned();
        let desc = order.get("direction").and_then(Value::as_str) == Some("desc");
        Some((
            property,
            if desc {
                Direction::Desc
            } else {
                Direction::Asc
            },
        ))
    });
    let tail = tail
        .iter()
        .map(|(property, direction)| ((*property).to_owned(), *direction));
    Sorting(given.chain(tail).collect())
}

/// Whether `resource` matches every one of `criterias` (`testStandardCriterias`): each is an
/// object whose every property matches the resource's, and an array of values is an alternative.
///
/// An error is the reference's `InvalidCriteria` or `UnsupportedValue`, which no controller
/// catches: the route answers 500.
pub fn matches(resource: &Value, criterias: &[Criteria]) -> Result<bool, ApiError> {
    let Value::Object(resource) = resource else {
        return Ok(false);
    };
    for criteria in criterias {
        if !properties(resource, from_map(&criteria.0))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A criterion's `Object.keys` and the criterion at each.
type Entries<'a> = Vec<(Cow<'a, str>, Cow<'a, Value>)>;

fn from_map(map: &Map<String, Value>) -> Entries<'_> {
    property_order(map)
        .into_iter()
        .map(|(key, value)| (Cow::Borrowed(key.as_str()), Cow::Borrowed(value)))
        .collect()
}

/// `Object.keys(criterion)` with the criteria they hold; `None` where that throws.
fn entries(criterion: &Value) -> Option<Entries<'_>> {
    Some(match criterion {
        Value::Object(map) => from_map(map),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(i, v)| (Cow::Owned(i.to_string()), Cow::Borrowed(v)))
            .collect(),
        Value::String(text) => {
            let chars = text.chars().enumerate();
            chars
                .map(|(i, c)| {
                    (
                        Cow::Owned(i.to_string()),
                        Cow::Owned(Value::String(c.to_string())),
                    )
                })
                .collect()
        }
        Value::Null => return None,
        _ => Vec::new(),
    })
}

/// `value[key]`; an inherited function is a value the reference cannot test.
fn member<'a>(map: &'a Map<String, Value>, key: &str) -> Result<Option<&'a Value>, ApiError> {
    match map.get(key) {
        None if INHERITED.contains(&key) => Err(ApiError::internal()),
        found => Ok(found),
    }
}

/// `testStandardCriterias(value, criterion)`: an array is the alternative of its items.
fn test(value: Option<&Value>, criterion: &Value) -> Test {
    let Value::Array(items) = criterion else {
        return item(value, criterion);
    };
    for one in items {
        if item(value, one)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `testStandardCriteriaItem`: by the value's type.
fn item(value: Option<&Value>, criterion: &Value) -> Test {
    let Some(value) = value else { return Ok(false) };
    match value {
        Value::Null => Ok(false),
        Value::Bool(flag) => boolean(*flag, criterion),
        Value::String(value) => string(value, criterion),
        Value::Number(number) => {
            numeric(heartwood_crypto::utils::js::number_value(number), criterion)
        }
        Value::Array(_) => Err(ApiError::internal()),
        Value::Object(map) => match marker(map) {
            Some((BIG_NUMBER, digits)) => big(digits, criterion),
            Some((_, version)) => semver(version, criterion),
            None => properties(map, entries(criterion).ok_or_else(ApiError::internal)?),
        },
    }
}

fn boolean(value: bool, criterion: &Value) -> Test {
    match criterion {
        Value::Bool(wanted) => Ok(*wanted == value),
        Value::String(wanted) if wanted == "true" || wanted == "false" => {
            Ok((wanted == "true") == value)
        }
        _ => Err(ApiError::internal()),
    }
}

fn string(value: &str, criterion: &Value) -> Test {
    let Value::String(criterion) = criterion else {
        return Err(ApiError::internal());
    };
    if !criterion.contains('%') {
        return Ok(criterion == value);
    }
    let mut from = 0;
    for part in criterion.split('%') {
        match value[from..].find(part) {
            Some(at) => from += at + part.len(),
            None => return Ok(false),
        }
    }
    Ok(true)
}

/// A range's test: `from` and `to` are read as the reference reads them, `to` only when `from`
/// held; an object with neither, and any other criterion, throws.
fn ranged(criterion: &Value, from: impl Fn(&Value) -> Test, to: impl Fn(&Value) -> Test) -> Test {
    let Value::Object(range) = criterion else {
        return Err(ApiError::internal());
    };
    match (range.get("from"), range.get("to")) {
        (Some(low), Some(high)) => Ok(from(low)? && to(high)?),
        (Some(low), None) => from(low),
        (None, Some(high)) => to(high),
        (None, None) => Err(ApiError::internal()),
    }
}

fn numeric(value: f64, criterion: &Value) -> Test {
    let read = |bound: &Value| match number(bound) {
        bound if bound.is_nan() => Err(ApiError::internal()),
        bound => Ok(bound),
    };
    match criterion {
        Value::String(_) | Value::Number(_) => Ok(value == read(criterion)?),
        Value::Object(range) => {
            // Both bounds are read before either is compared.
            let bounds = (
                range.get("from").map(read).transpose()?,
                range.get("to").map(read).transpose()?,
            );
            if bounds == (None, None) {
                return Err(ApiError::internal());
            }
            Ok(
                bounds.0.is_none_or(|low| value >= low)
                    && bounds.1.is_none_or(|high| value <= high),
            )
        }
        _ => Err(ApiError::internal()),
    }
}

fn big(digits: &str, criterion: &Value) -> Test {
    let value = Big::parse(digits).ok_or_else(ApiError::internal)?;
    let of = |bound: &Value| Big::of(bound).ok_or_else(ApiError::internal);
    match criterion {
        Value::String(_) | Value::Number(_) => Ok(value == of(criterion)?),
        _ => ranged(
            criterion,
            |low| Ok(value >= of(low)?),
            |high| Ok(value <= of(high)?),
        ),
    }
}

/// `semver.clean`: without the white space and the `=` and `v` in front, in its normal form.
fn clean(version: &Value) -> Option<String> {
    let version = text(version);
    npm_semver::valid(
        version
            .trim_matches(is_space)
            .trim_start_matches(['=', 'v']),
    )
}

fn semver(value: &str, criterion: &Value) -> Test {
    let against = |bound: &Value| {
        let bound = clean(bound).ok_or_else(ApiError::internal)?;
        npm_semver::compare(value, &bound).ok_or_else(ApiError::internal)
    };
    match criterion {
        Value::String(_) => Ok(against(criterion)? == Ordering::Equal),
        _ => ranged(
            criterion,
            |low| Ok(against(low)? != Ordering::Less),
            |high| Ok(against(high)? != Ordering::Greater),
        ),
    }
}

/// `testObjectValueCriteriaItem`: `{"*": criterion}` tests any of the values; else every property
/// of the criterion tests the value's property of the same name.
fn properties(value: &Map<String, Value>, criterion: Entries<'_>) -> Test {
    if let [(key, wildcard)] = criterion.as_slice()
        && key == "*"
    {
        for (_, one) in property_order(value) {
            if test(Some(one), wildcard)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    for (key, wanted) in &criterion {
        let held = member(value, key)?;
        // A wallet's `votingFor` tests for a delegate's name as `[{name: "*"}]`.
        let voted = match (key.as_ref(), held, wanted.as_ref()) {
            ("votingFor", Some(Value::Object(votes)), Value::Array(items)) => match items.first() {
                Some(Value::Object(first)) => match property_order(first).first() {
                    Some((name, Value::String(any))) if any == "*" => {
                        Some(votes.contains_key(*name))
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        if !voted.map_or_else(|| test(held, wanted), Ok)? {
            return Ok(false);
        }
    }
    Ok(true)
}
