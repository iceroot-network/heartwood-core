//! The payload of a `POST`, read as hapi reads it, in its order: a declared length over the limit;
//! the content type (JSON when none is given, or an empty one), which must read as
//! `type/subtype`, with a boundary for a multipart type; a multipart form, which hapi does not
//! parse; the body, up to the limit; then by type: JSON (`null` when empty; refused when it does
//! not parse or names `__proto__`), text, bytes (`null` when empty, else read as text: the
//! schema refuses both), a form (read as no fields, which the schema refuses too), or any other
//! type refused. The reference answers every failure with a 422 and hapi's message (its server's
//! `failAction` for a payload).

use axum::body::{Body, to_bytes};
use axum::http::HeaderMap;
use axum::http::header::{self, HeaderName};
use serde_json::{Map, Value};

use crate::errors::ApiError;

/// The largest request body, hapi's default.
const MAX_BODY: usize = 1_048_576;

/// The payload of a request with `headers` and `body`.
pub(super) async fn read(headers: &HeaderMap, body: Body) -> Result<Value, ApiError> {
    let refused = |message: &str| ApiError::unprocessable(message);
    let too_large = || {
        let message = format!("Payload content length greater than maximum allowed: {MAX_BODY}");
        ApiError::unprocessable(message)
    };
    let text_of = |name: HeaderName| headers.get(name).and_then(|value| value.to_str().ok());
    let declared = text_of(header::CONTENT_LENGTH).and_then(|text| text.trim().parse::<u64>().ok());
    if declared.is_some_and(|length| length > MAX_BODY as u64) {
        return Err(too_large());
    }
    let given = text_of(header::CONTENT_TYPE).filter(|text| !text.is_empty());
    let mime = media_type(given.unwrap_or("application/json")).map_err(refused)?;
    let unsupported = || refused("Unsupported Media Type");
    if mime == "multipart/form-data" {
        return Err(unsupported());
    }
    let bytes = to_bytes(body, MAX_BODY).await.map_err(|_| too_large())?;
    // The body is read as UTF-8 text, a sequence that is not UTF-8 becoming U+FFFD.
    let text = String::from_utf8_lossy(&bytes);
    // `application/json`, or `application/<type>+json`.
    let json = mime.strip_prefix("application/");
    let json = json.and_then(|subtype| subtype.strip_suffix("json"));
    let json =
        json.is_some_and(|middle| middle.is_empty() || middle.len() > 1 && middle.ends_with('+'));
    match mime.as_str() {
        _ if (json || mime == "application/octet-stream") && bytes.is_empty() => Ok(Value::Null),
        _ if json => match serde_json::from_str::<Value>(&text) {
            Ok(value) if !names_proto(&value) => Ok(value),
            _ => Err(refused("Invalid request payload JSON format")),
        },
        "application/octet-stream" => Ok(Value::String(text.into_owned())),
        "application/x-www-form-urlencoded" => Ok(Value::Object(Map::new())),
        mime if mime.len() > 5 && mime.starts_with("text/") => Ok(Value::String(text.into_owned())),
        _ => Err(unsupported()),
    }
}

/// The media type of a `content-type` header as hapi's parser reads it, lowercase: a type (no
/// `/` or space), `/`, and a subtype up to a space or `;`, whatever follows; a multipart type
/// needs a `boundary` parameter.
fn media_type(header: &str) -> Result<String, &'static str> {
    let invalid = "Invalid content-type header";
    let split = header.find(|c: char| c == '/' || c.is_whitespace());
    let (kind, rest) = header.split_at(split.ok_or(invalid)?);
    let subtype = rest.strip_prefix('/').ok_or(invalid)?;
    let end = subtype.find(|c: char| c == ';' || c.is_whitespace());
    let (subtype, parameters) = subtype.split_at(end.unwrap_or(subtype.len()));
    if kind.is_empty() || subtype.is_empty() {
        return Err(invalid);
    }
    let mime = format!("{kind}/{subtype}").to_ascii_lowercase();
    if mime.starts_with("multipart/") && !has_boundary(parameters) {
        return Err("Invalid content-type header: multipart missing boundary");
    }
    Ok(mime)
}

/// Whether `parameters` give a boundary: `;`, spaces, `boundary=` in any case, and a quoted value
/// or a value without `;`, `"` or spaces.
fn has_boundary(parameters: &str) -> bool {
    parameters.match_indices(';').any(|(at, _)| {
        let rest = parameters.get(at + 1..).unwrap_or_default();
        let rest = rest.trim_start_matches(char::is_whitespace);
        let name = rest
            .get(..9)
            .filter(|name| name.eq_ignore_ascii_case("boundary="));
        let Some(value) = name.and(rest.get(9..)) else {
            return false;
        };
        match value.strip_prefix('"') {
            Some(quoted) => quoted.find('"').is_some_and(|end| end > 0),
            None => value
                .chars()
                .next()
                .is_some_and(|c| c != ';' && c != '"' && !c.is_whitespace()),
        }
    })
}

/// Whether an object in `value` has a key `__proto__`, which the reference's JSON parser refuses.
fn names_proto(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("__proto__") || map.values().any(names_proto),
        Value::Array(items) => items.iter().any(names_proto),
        _ => false,
    }
}
