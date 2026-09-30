//! What the state listings' tests share: a query as the route validates it, a request of it, and
//! a stand-in for the pagination service (a stable sort by the reference's comparator) so that the
//! listings are tested before and apart from the service itself.

#![allow(dead_code)]

use std::cmp::Ordering;
use std::net::{IpAddr, Ipv4Addr};

use heartwood_api::pagination::Pagination;
use heartwood_api::schemas::{parse_query, validate_query};
use heartwood_api::search::{Direction, ResultsPage, Sorting};
use heartwood_api::{ApiError, ApiRequest, RouteId};
use heartwood_p2p::utils::npm_semver;
use serde_json::{Map, Value};

/// The query `raw` as `route` validates it.
pub fn query(route: RouteId, raw: &str) -> Result<Value, ApiError> {
    validate_query(route, &parse_query(raw))
}

/// A request of `route` with the validated `query`.
pub fn request(route: RouteId, id: Option<&str>, query: Value) -> ApiRequest {
    let mut params = Map::new();
    let mut path = route.path().to_owned();
    if let Some(id) = id {
        params.insert("id".to_owned(), Value::from(id));
        path = path.replace("{id}", id);
    }
    ApiRequest {
        route,
        params,
        path,
        query,
        payload: None,
        client: IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |value, key| value.get(key))
}

fn ordering(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::String(a), Value::String(b)) => Some(a.encode_utf16().cmp(b.encode_utf16())),
        (Value::Number(a), Value::Number(b)) => a.as_f64()?.partial_cmp(&b.as_f64()?),
        (Value::Object(a), Value::Object(b)) => {
            let text =
                |map: &Map<String, Value>, key: &str| map.get(key)?.as_str().map(str::to_owned);
            match (text(a, "$bigNumber"), text(b, "$bigNumber")) {
                (Some(a), Some(b)) => Some(a.parse::<i128>().ok()?.cmp(&b.parse::<i128>().ok()?)),
                _ => npm_semver::compare(&text(a, "$semver")?, &text(b, "$semver")?),
            }
        }
        _ => None,
    }
}

/// The reference's comparator: absent values, then `null`, last in both directions; the values
/// of one type in the direction; any other pair is its error.
fn compare(a: &Value, b: &Value, sorting: &Sorting) -> Result<Ordering, ApiError> {
    for (property, direction) in &sorting.0 {
        let (a, b) = (get(a, property), get(b, property));
        match (a, b) {
            (None, None) => continue,
            (None, Some(_)) => return Ok(Ordering::Greater),
            (Some(_), None) => return Ok(Ordering::Less),
            (Some(Value::Null), Some(Value::Null)) => continue,
            (Some(Value::Null), Some(_)) => return Ok(Ordering::Greater),
            (Some(_), Some(Value::Null)) => return Ok(Ordering::Less),
            (Some(a), Some(b)) => {
                let (a, b) = if *direction == Direction::Desc {
                    (b, a)
                } else {
                    (a, b)
                };
                match ordering(a, b) {
                    Some(Ordering::Equal) => continue,
                    Some(order) => return Ok(order),
                    None => return Err(ApiError::internal()),
                }
            }
        }
    }
    Ok(Ordering::Equal)
}

/// The page of `items` in the order of `sorting`, as the pagination service gives it.
pub fn stand_in(
    pagination: Pagination,
    sorting: &Sorting,
    items: std::vec::IntoIter<Value>,
) -> Result<ResultsPage, ApiError> {
    let all: Vec<Value> = items.collect();
    let mut failed = false;
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by(|a, b| match compare(&all[*a], &all[*b], sorting) {
        Ok(order) => order.then(a.cmp(b)),
        Err(_) => {
            failed = true;
            Ordering::Equal
        }
    });
    if failed {
        return Err(ApiError::internal());
    }
    let skip = usize::try_from(pagination.offset).unwrap_or(usize::MAX);
    let take = usize::try_from(pagination.limit).unwrap_or(usize::MAX);
    Ok(ResultsPage {
        results: order
            .into_iter()
            .skip(skip)
            .take(take)
            .map(|index| all[index].clone())
            .collect(),
        total_count: all.len() as u64,
        total_count_is_estimate: false,
    })
}
