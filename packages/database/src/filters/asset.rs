//! The reference's expansion of an `asset` criteria: a query value is text, while the stored
//! asset has numbers and booleans, so each text that reads as one also stands for it.

use heartwood_crypto::utils::js::number_to_string;
use serde_json::{Number, Value};

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
            let choices = items.iter().map(|item| alternatives(item, left));
            let choices = choices.collect::<Result<Vec<_>, _>>()?;
            Ok(combinations(&choices).map(Value::Array).collect())
        }
        Value::Object(map) => {
            let choices = map.values().map(|inner| alternatives(inner, left));
            let choices = choices.collect::<Result<Vec<_>, _>>()?;
            let object =
                |picked: Vec<Value>| Value::Object(map.keys().cloned().zip(picked).collect());
            Ok(combinations(&choices).map(object).collect())
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

/// Every pick of one choice from each list of `choices`, the first list's choice varying slowest,
/// as the reference spreads each prefix over each choice in turn. Each list holds two
/// alternatives for each of its casts, so there are at most 32 picks, each built in one pass.
fn combinations(choices: &[Vec<Value>]) -> impl Iterator<Item = Vec<Value>> + '_ {
    let count = choices.iter().map(Vec::len).fold(1, usize::saturating_mul);
    (0..count).map(move |index| {
        let mut rest = index;
        let mut picked: Vec<Value> = choices
            .iter()
            .rev()
            .filter_map(|list| {
                let len = list.len().max(1);
                let choice = list.get(rest % len).cloned();
                rest /= len;
                choice
            })
            .collect();
        picked.reverse();
        picked
    })
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

    /// The reference's expansion as written: each prefix spread over each item's choices.
    fn spread(value: &Value, left: &mut u32) -> Result<Vec<Value>, CastLimit> {
        let extend = |prefixes: Vec<Vec<(String, Value)>>, key: &str, choices: Vec<Value>| {
            let mut out = Vec::new();
            for prefix in prefixes {
                for choice in &choices {
                    let mut next = prefix.clone();
                    next.push((key.to_owned(), choice.clone()));
                    out.push(next);
                }
            }
            out
        };
        let entries: Vec<(String, &Value)> = match value {
            Value::Array(items) => items.iter().map(|item| (String::new(), item)).collect(),
            Value::Object(map) => map
                .iter()
                .map(|(key, inner)| (key.clone(), inner))
                .collect(),
            _ => return alternatives(value, left),
        };
        let mut out = vec![Vec::new()];
        for (key, inner) in entries {
            out = extend(out, &key, spread(inner, left)?);
        }
        let rebuild = |pairs: Vec<(String, Value)>| match value {
            Value::Array(_) => Value::Array(pairs.into_iter().map(|(_, v)| v).collect()),
            _ => Value::Object(pairs.into_iter().collect()),
        };
        Ok(out.into_iter().map(rebuild).collect())
    }

    #[test]
    fn the_expansion_equals_the_reference_spread_in_content_and_order() {
        let criteria = [
            json!([]),
            json!({}),
            json!(["1", ["2", {"b": "true", "a": ["x", "3"]}], "y"]),
            json!({"z": "false", "a": [null, "4", {"k": ["5"]}], "m": 7}),
            json!({"a": ["1", "2", "3", "true", "false", null, "c"]}),
            json!([[[["1"]]], "2", [], {}, "NaN"]),
        ];
        for criterion in criteria {
            let mut left = CASTS;
            assert_eq!(
                asset_alternatives(&criterion),
                spread(&criterion, &mut left),
                "{criterion}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_types,
        reason = "the test times the expansion against the wall clock"
    )]
    fn a_long_list_with_casts_expands_in_linear_time() {
        // `asset.x=1,2,3,true,false&asset.x.16383=c`: five casts and 16,379 holes.
        let mut items = vec![json!("1"), json!("2"), json!("3"), json!("true")];
        items.push(json!("false"));
        items.resize(16_383, Value::Null);
        items.push(json!("c"));
        let criterion = json!({"x": items});
        let started = std::time::Instant::now();
        let expanded = asset_alternatives(&criterion).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(expanded.len(), 32);
        let first = |value: &Value| value["x"].as_array().unwrap()[..6].to_vec();
        assert_eq!(first(&expanded[0])[..5], items[..5]);
        let cast = [
            json!(1.0),
            json!(2.0),
            json!(3.0),
            json!(true),
            json!(false),
        ];
        assert_eq!(first(&expanded[31])[..5], cast);
        assert_eq!(expanded[0]["x"][16_383], json!("c"));
        assert!(elapsed.as_secs() < 2, "{elapsed:?}");
    }

    #[test]
    fn more_than_five_casts_are_refused() {
        let five = json!({"a": "1", "b": "2", "c": "3", "d": "true", "e": "false", "f": "x"});
        assert_eq!(asset_alternatives(&five).map(|v| v.len()), Ok(32));
        let six = json!({"a": "1", "b": "2", "c": "3", "d": "true", "e": "false", "f": "6"});
        assert_eq!(asset_alternatives(&six), Err(CastLimit));
    }
}
