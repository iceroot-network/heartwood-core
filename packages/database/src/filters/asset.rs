//! The reference's expansion of an `asset` criteria: a query value is text, while the stored
//! asset has numbers and booleans, so each text that reads as one also stands for it.

use heartwood_crypto::utils::js::number_to_string;
use serde_json::{Map, Number, Value};

use crate::filters::js_number;

/// The most texts of one `asset` criteria that stand for a number or a boolean as well.
const CASTS: u32 = 5;

/// An `asset` criteria with more texts that read as a number or a boolean than the reference
/// expands: it fails the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Asset cast property limit reached")]
pub struct CastLimit;

/// Every value that the `asset` criteria `criterion` stands for, as the reference expands it: a
/// text that JavaScript reads as a number and writes back unchanged (`String(Number(text)) ===
/// text`) stands for itself and for that number, `"true"` and `"false"` for themselves and the
/// boolean, in objects and arrays at any depth, every combination once. A number that is not
/// finite (`NaN`, `Infinity`) becomes `null`, as the reference's JSON parameter writes it. More
/// than five such texts are refused.
pub fn asset_alternatives(criterion: &Value) -> Result<Vec<Value>, CastLimit> {
    let mut left = CASTS;
    alternatives(criterion, &mut left)
}

/// The alternatives of `value`, counting the casts in `left`.
fn alternatives(value: &Value, left: &mut u32) -> Result<Vec<Value>, CastLimit> {
    match value {
        Value::Array(items) => {
            let mut out: Vec<Vec<Value>> = vec![Vec::new()];
            for item in items {
                let choices = alternatives(item, left)?;
                out = product(out, &choices, |prefix, choice| prefix.push(choice));
            }
            Ok(out.into_iter().map(Value::Array).collect())
        }
        Value::Object(map) => {
            let mut out: Vec<Map<String, Value>> = vec![Map::new()];
            for (key, inner) in map {
                let choices = alternatives(inner, left)?;
                out = product(out, &choices, |prefix, choice| {
                    prefix.insert(key.clone(), choice);
                });
            }
            Ok(out.into_iter().map(Value::Object).collect())
        }
        Value::String(text) => match cast(text) {
            Some(cast) => {
                *left = left.checked_sub(1).ok_or(CastLimit)?;
                Ok(vec![value.clone(), cast])
            }
            None => Ok(vec![value.clone()]),
        },
        _ => Ok(vec![value.clone()]),
    }
}

/// Every `prefix` extended by every `choice`.
fn product<T: Clone>(
    prefixes: Vec<T>,
    choices: &[Value],
    extend: impl Fn(&mut T, Value),
) -> Vec<T> {
    let mut out = Vec::with_capacity(prefixes.len().saturating_mul(choices.len()));
    for prefix in prefixes {
        for choice in choices {
            let mut next = prefix.clone();
            extend(&mut next, choice.clone());
            out.push(next);
        }
    }
    out
}

/// The number or boolean that `text` also stands for, if any.
fn cast(text: &str) -> Option<Value> {
    let number = js_number(text);
    if number_to_string(number) == text {
        return Some(Number::from_f64(number).map_or(Value::Null, Value::Number));
    }
    match text {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn texts_that_read_back_unchanged_also_stand_for_numbers() {
        assert_eq!(
            asset_alternatives(&json!({"votes": {"alice": "50"}})),
            Ok(vec![
                json!({"votes": {"alice": "50"}}),
                json!({"votes": {"alice": 50.0}})
            ])
        );
        for text in ["0.5", "-1", "1e+21", "NaN", "Infinity"] {
            assert_eq!(
                asset_alternatives(&json!(text)).map(|v| v.len()),
                Ok(2),
                "{text}"
            );
        }
        for text in ["050", "1.50", "1e3", "", " 5", "-0", "0x10", "abc"] {
            assert_eq!(
                asset_alternatives(&json!(text)).map(|v| v.len()),
                Ok(1),
                "{text}"
            );
        }
        assert_eq!(
            asset_alternatives(&json!("NaN")),
            Ok(vec![json!("NaN"), json!(null)])
        );
    }

    #[test]
    fn booleans_and_arrays_expand_in_every_combination() {
        assert_eq!(
            asset_alternatives(&json!({"a": ["true", "x", "2"]})),
            Ok(vec![
                json!({"a": ["true", "x", "2"]}),
                json!({"a": ["true", "x", 2.0]}),
                json!({"a": [true, "x", "2"]}),
                json!({"a": [true, "x", 2.0]}),
            ])
        );
    }

    #[test]
    fn more_than_five_casts_are_refused() {
        let five = json!({"a": "1", "b": "2", "c": "3", "d": "true", "e": "false", "f": "x"});
        assert_eq!(asset_alternatives(&five).map(|v| v.len()), Ok(32));
        let six = json!({"a": "1", "b": "2", "c": "3", "d": "true", "e": "false", "f": "6"});
        assert_eq!(asset_alternatives(&six), Err(CastLimit));
    }
}
