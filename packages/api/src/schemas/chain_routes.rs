//! The schemas of the chain's routes: `GET /blockchain`, `GET /blockchain/search/{id}`'s
//! parameter, `GET /node/fees`'s `days` and `GET /transactions/schemas`.
//!
//! `days` is a plain Joi number (`Joi.number().integer().min(1).max(30)`), so a refusal carries
//! Joi's own message for the rule that failed rather than the alternatives' shared one.

use serde_json::{Map, Value};

use super::{BEYOND, SAFE, is_js_space, joi_number};
use crate::context::RouteId;
use crate::errors::ApiError;

/// The longest id the search route takes.
const MOST_SEARCH_ID: usize = 66;

/// The query of `route`, validated: defaults added and values coerced, or 422. Only `GET
/// /node/fees` has a schema (`days`, an integer from 1 to 30); the other routes take any query.
pub fn validate_query(route: RouteId, query: &Value) -> Result<Value, ApiError> {
    let Value::Object(input) = query else {
        return Err(ApiError::internal());
    };
    let mut input = input.clone();
    let beyond = input.shift_remove(BEYOND).is_some();
    if route != RouteId::NodeFees {
        return Ok(Value::Object(input));
    }
    let mut out = input.clone();
    if let Some(days) = input.get("days") {
        out.insert("days".into(), Value::from(days_of(days)?));
    }
    if let Some(unknown) = input.keys().find(|name| name.as_str() != "days") {
        return Err(ApiError::unprocessable(format!(
            "\"{unknown}\" is not allowed"
        )));
    }
    // Only the asset criteria accept a value that the grammar did not write out whole.
    if beyond {
        return Err(ApiError::internal());
    }
    Ok(Value::Object(out))
}

/// Validate the path parameters of `route`: the search route's `id` matches
/// `^[a-zA-Z0-9!@$&_.]{1,66}$`.
pub fn validate_params(route: RouteId, params: &Map<String, Value>) -> Result<(), ApiError> {
    if route != RouteId::BlockchainSearch {
        return Ok(());
    }
    for (key, value) in params {
        if key != "id" {
            return Err(ApiError::unprocessable(format!("\"{key}\" is not allowed")));
        }
        let text = value.as_str().unwrap_or_default();
        if text.is_empty() {
            return Err(ApiError::unprocessable("\"id\" is not allowed to be empty"));
        }
        let allowed = |byte: u8| byte.is_ascii_alphanumeric() || b"!@$&_.".contains(&byte);
        if text.len() > MOST_SEARCH_ID || !text.bytes().all(allowed) {
            return Err(ApiError::unprocessable(format!(
                "\"id\" with value \"{text}\" fails to match the required pattern: \
                 /^[a-zA-Z0-9!@$&_.]{{1,66}}$/"
            )));
        }
    }
    Ok(())
}

/// The value of `days` as Joi's `number().integer().min(1).max(30)` reads it, or the message of
/// the rule that refuses it.
fn days_of(value: &Value) -> Result<i64, ApiError> {
    let refuse = |rule: &str| ApiError::unprocessable(format!("\"days\" {rule}"));
    let Some(text) = value.as_str().filter(|text| shaped_like_number(text)) else {
        return Err(refuse("must be a number"));
    };
    let Some(number) = joi_number(text) else {
        return Err(refuse("must be a safe number"));
    };
    if number.abs() > SAFE as f64 {
        return Err(refuse("must be a safe number"));
    }
    if number.trunc() != number {
        return Err(refuse("must be an integer"));
    }
    if number < 1.0 {
        return Err(refuse("must be greater than or equal to 1"));
    }
    if number > 30.0 {
        return Err(refuse("must be less than or equal to 30"));
    }
    // A whole number from 1 to 30 converts exactly.
    Ok(number as i64)
}

/// Whether `text` is what Joi reads as a number at all: white space, an optional sign, digits with
/// an optional fraction (or a fraction alone), and an optional exponent.
fn shaped_like_number(text: &str) -> bool {
    let trimmed = text.trim_matches(is_js_space);
    let (mantissa, exponent) = match trimmed.find(['e', 'E']) {
        Some(at) => (&trimmed[..at], Some(&trimmed[at + 1..])),
        None => (trimmed, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let unsigned = mantissa.strip_prefix(['+', '-']).unwrap_or(mantissa);
    let mantissa_fits = match unsigned.split_once('.') {
        Some((whole, fraction)) => {
            (digits(whole) && (fraction.is_empty() || digits(fraction)))
                || (whole.is_empty() && digits(fraction))
        }
        None => digits(unsigned),
    };
    mantissa_fits
        && exponent.is_none_or(|part| digits(part.strip_prefix(['+', '-']).unwrap_or(part)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn days(value: Value) -> Result<Value, String> {
        validate_query(RouteId::NodeFees, &json!({ "days": value })).map_err(|e| {
            e.body()["message"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_default()
        })
    }

    #[test]
    fn days_is_read_as_joi_reads_a_bounded_integer() {
        assert_eq!(days(json!("7")), Ok(json!({ "days": 7 })));
        assert_eq!(days(json!(" 30.0 ")), Ok(json!({ "days": 30 })));
        assert_eq!(days(json!("1.5e1")), Ok(json!({ "days": 15 })));
        assert_eq!(days(json!("5.")), Ok(json!({ "days": 5 })));
        assert_eq!(days(json!("00007")), Ok(json!({ "days": 7 })));
    }

    #[test]
    fn days_is_refused_with_the_message_of_the_failing_rule() {
        let message = |value: Value| days(value).unwrap_err();
        assert_eq!(
            message(json!("0")),
            "\"days\" must be greater than or equal to 1"
        );
        assert_eq!(
            message(json!("31")),
            "\"days\" must be less than or equal to 30"
        );
        assert_eq!(message(json!("1.5")), "\"days\" must be an integer");
        assert_eq!(message(json!("abc")), "\"days\" must be a number");
        assert_eq!(message(json!("")), "\"days\" must be a number");
        assert_eq!(message(json!(["1", "2"])), "\"days\" must be a number");
        assert_eq!(message(json!("1e16")), "\"days\" must be a safe number");
        assert_eq!(
            message(json!("9007199254740992")),
            "\"days\" must be a safe number"
        );
        assert_eq!(
            message(json!("1e15")),
            "\"days\" must be less than or equal to 30"
        );
    }

    #[test]
    fn the_fees_query_refuses_other_keys_and_the_other_routes_take_any() {
        let error = validate_query(RouteId::NodeFees, &json!({ "page": "1" })).unwrap_err();
        assert_eq!(error.body()["message"], "\"page\" is not allowed");
        assert_eq!(validate_query(RouteId::NodeFees, &json!({})), Ok(json!({})));
        let any = json!({ "anything": "goes" });
        assert_eq!(
            validate_query(RouteId::BlockchainIndex, &any),
            Ok(any.clone())
        );
        assert_eq!(validate_query(RouteId::TransactionsSchemas, &any), Ok(any));
    }

    #[test]
    fn the_search_id_matches_the_route_pattern() {
        let check = |id: &str| {
            let mut params = Map::new();
            params.insert("id".into(), Value::from(id));
            validate_params(RouteId::BlockchainSearch, &params)
        };
        assert!(check("Abc!@$&_.9").is_ok());
        assert!(check(&"a".repeat(66)).is_ok());
        assert!(check(&"a".repeat(67)).is_err());
        let error = check("a-b").unwrap_err();
        assert_eq!(
            error.body()["message"],
            "\"id\" with value \"a-b\" fails to match the required pattern: /^[a-zA-Z0-9!@$&_.]{1,66}$/"
        );
        assert_eq!(
            check("").unwrap_err().body()["message"],
            "\"id\" is not allowed to be empty"
        );
    }
}
