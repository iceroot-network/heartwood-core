//! The list envelope: `meta` (`totalCountIsEstimate`, `count`, `pageCount`, `totalCount`, `next`,
//! `previous`, `self`, `first`, `last`, in that order) and `data`, with the reference's link
//! strings. `offset=0` is ignored, as the reference ignores it; another offset replaces the page.
//!
//! A link is the list's path under the base path, as hapi's request URL writes it (the escapes of
//! unreserved characters decoded, escapes in upper case, the characters a URL's path cannot hold
//! escaped), `?`, and the query the reference renders: the validated query without `orderBy`, overlaid with the query as parsed (so a given `orderBy` is
//! kept and comes last, and a given value keeps its spelling, while `page`, `limit` and
//! `transform` appear with their defaults when they were not given), with the link's page, written
//! as `qs.stringify` writes it with dots for nesting, commas for lists and no escaping.

use heartwood_crypto::utils::js;
use serde_json::{Map, Value};

use crate::context::ApiRequest;
use crate::schemas::PARSED;

/// The default base path, which a path given by a caller outside the server may still carry.
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
/// the reference. `request.path` is the path under the base path, as the server gives it; a path
/// that starts with the default base path `/api` has it taken off.
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
        let path = url_path(path);
        let path = match path.strip_suffix('/') {
            Some(stripped) if !stripped.is_empty() => stripped,
            _ => &path,
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

/// `path` as hapi's request URL writes it: a byte a URL's path cannot hold (a space, `"`, `<`,
/// `>`, a backquote, a brace, a control or a byte beyond ASCII) escaped, then every escape in upper
/// case and those of unreserved characters (letters, digits and `!$&'()*+,-.:;=@_~`) decoded.
fn url_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let hex = bytes.get(index + 1..index + 3).filter(|_| byte == b'%');
        let hex = hex.filter(|hex| hex.iter().all(u8::is_ascii_hexdigit));
        let escaped =
            hex.and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(value)
                if value.is_ascii_alphanumeric() || b"!$&'()*+,-.:;=@_~".contains(&value) =>
            {
                out.push(char::from(value));
                index += 3;
            }
            Some(value) => {
                out.push_str(&format!("%{value:02X}"));
                index += 3;
            }
            None if byte <= b' ' || byte >= 0x7f || b"\"<>`{}".contains(&byte) => {
                out.push_str(&format!("%{byte:02X}"));
                index += 1;
            }
            None => {
                out.push(char::from(byte));
                index += 1;
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::url_path;

    #[test]
    fn a_link_path_is_written_as_hapi_writes_its_request_url() {
        for (path, written) in [
            ("/transactions", "/transactions"),
            (
                "/wallets/a%5fb%2A%7e/transactions",
                "/wallets/a_b*~/transactions",
            ),
            (
                "/blocks/%2f%7c%25%zz%4/transactions",
                "/blocks/%2F%7C%25%zz%4/transactions",
            ),
            (
                "/blocks/a{b}\"`<>/transactions",
                "/blocks/a%7Bb%7D%22%60%3C%3E/transactions",
            ),
            ("/blocks/%+1/transactions", "/blocks/%+1/transactions"),
        ] {
            assert_eq!(url_path(path), written, "{path}");
        }
    }
}
