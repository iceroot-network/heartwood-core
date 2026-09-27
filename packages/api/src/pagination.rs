//! The list envelope: `meta` (`totalCountIsEstimate`, `count`, `pageCount`, `totalCount`, `next`,
//! `previous`, `self`, `first`, `last`, in that order) and `data`, with the reference's link
//! strings. `offset=0` is ignored, as the reference ignores it; another offset replaces the page.
//!
//! A link is the list's path under the base path, `?`, and the query the reference renders: the
//! validated query without `orderBy`, overlaid with the query as parsed (so a given `orderBy` is
//! kept and comes last, and a given value keeps its spelling, while `page`, `limit` and
//! `transform` appear with their defaults when they were not given), with the link's page, written
//! as `qs.stringify` writes it with dots for nesting, commas for lists and no escaping.

use heartwood_crypto::utils::js;
use serde_json::{Map, Value};

use crate::context::ApiRequest;
use crate::schemas::PARSED;

/// The base path the links are relative to.
const BASE_PATH: &str = "/api";

/// A page of a list route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// The matches skipped.
    pub offset: u64,
    /// The most matches returned.
    pub limit: u64,
}

impl Pagination {
    /// The page that the validated query `query` asks for (`page`, `limit`, `offset`): the
    /// matches before the page are skipped, unless a non-zero `offset` says how many to skip.
    pub fn of(query: &Value) -> Pagination {
        let positive = |name: &str| query.get(name).and_then(Value::as_u64).filter(|n| *n > 0);
        let limit = positive("limit").unwrap_or(100);
        let page = positive("page").unwrap_or(1);
        let offset = positive("offset").unwrap_or_else(|| (page - 1).saturating_mul(limit));
        Pagination { offset, limit }
    }
}

/// The list envelope of `data`, the page `pagination` of `total_count` matches of `request`.
///
/// The page numbers come from the query's `page` even when an offset chose the matches, as in
/// the reference. `request.path` is the path as requested; a leading `/api` is taken off, so a
/// path given under the base path already is used as it is.
pub fn envelope(
    request: &ApiRequest,
    pagination: Pagination,
    total_count: u64,
    total_count_is_estimate: bool,
    data: Vec<Value>,
) -> Value {
    let page = request
        .query
        .get("page")
        .and_then(Value::as_u64)
        .filter(|p| *p > 0)
        .unwrap_or(1);
    let limit = pagination.limit.max(1);
    let page_count = if total_count > 0 {
        total_count.div_ceil(limit)
    } else {
        1
    };
    let links = Links::of(request);
    let link = |p: u64| Value::String(links.at(p));
    let next = if total_count > 0 && page < page_count {
        link(page + 1)
    } else {
        Value::Null
    };
    let previous = if total_count > 0 && page > 1 && page <= page_count.saturating_add(1) {
        link(page - 1)
    } else {
        Value::Null
    };
    let mut meta = Map::new();
    meta.insert(
        "totalCountIsEstimate".into(),
        Value::Bool(total_count_is_estimate),
    );
    meta.insert("count".into(), Value::from(data.len()));
    meta.insert("pageCount".into(), Value::from(page_count));
    meta.insert("totalCount".into(), Value::from(total_count));
    meta.insert("next".into(), next);
    meta.insert("previous".into(), previous);
    meta.insert("self".into(), link(page));
    meta.insert("first".into(), link(1));
    meta.insert("last".into(), link(page_count));
    let mut body = Map::new();
    body.insert("meta".into(), Value::Object(meta));
    body.insert("data".into(), Value::Array(data));
    Value::Object(body)
}

/// What every link of a list shares: its path and its query without the page.
struct Links {
    path: String,
    query: Map<String, Value>,
}

impl Links {
    fn of(request: &ApiRequest) -> Links {
        let path = request
            .path
            .strip_prefix(BASE_PATH)
            .unwrap_or(&request.path);
        let path = match path.strip_suffix('/') {
            Some(stripped) if !stripped.is_empty() => stripped,
            _ => path,
        };
        let mut query = request.query.as_object().cloned().unwrap_or_default();
        let parsed = query.shift_remove(PARSED);
        query.shift_remove("orderBy");
        if let Some(Value::Object(parsed)) = parsed {
            for (key, value) in js::property_order(&parsed) {
                query.insert(key.clone(), value.clone());
            }
        }
        Links {
            path: path.to_owned(),
            query,
        }
    }

    /// The link to page `page`.
    fn at(&self, page: u64) -> String {
        let mut query = self.query.clone();
        query.insert("page".into(), Value::from(page));
        let mut pairs = Vec::new();
        for (key, value) in js::property_order(&query) {
            push_pairs(&mut pairs, key, value);
        }
        format!("{}?{}", self.path, pairs.join("&"))
    }
}

/// The pairs `qs.stringify` writes for `value` at `prefix`.
fn push_pairs(pairs: &mut Vec<String>, prefix: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in js::property_order(map) {
                push_pairs(pairs, &format!("{prefix}.{key}"), item);
            }
        }
        Value::Array(items) if items.is_empty() => {}
        _ => pairs.push(format!("{prefix}={}", text(value))),
    }
}

/// `value` as JavaScript turns it into text in a link: `String(value)`, lists joined by commas,
/// `null` as nothing.
fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js::number_to_string(js::number_value(number)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}
