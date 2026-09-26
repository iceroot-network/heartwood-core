//! The comparison engine: a structural JSON diff with byte-level detail for hex strings.
//!
//! Values are compared exactly (amounts are decimal strings, bytes are lowercase hex), with two
//! rules from JavaScript, where the oracle's values come from: numbers are equal when their double
//! values are (`8` and `8.0` are one number), and the keys of an object must come in the same
//! order, where the actual object's keys are taken in JavaScript's enumeration order (array-index
//! keys first, ascending, then the others in insertion order). A difference names its JSON path. When both sides are byte strings, it also names the first
//! differing byte offset and shows the bytes around it, so a codec mismatch points at the field.
//! Normalisers (for example for a defaulted `typeGroup`) are applied by the caller before
//! the diff, per operation, when the Rust side needs them.

use std::fmt;

use heartwood_crypto::utils::js;
use serde_json::Value;

/// Bytes of context shown on each side of the first differing byte.
const CONTEXT: usize = 8;
/// Longest scalar shown in full in a difference.
const MAX_SHOWN: usize = 96;

/// One difference between the expected (oracle) and the actual (Heartwood) value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// Two scalars, or two values of different JSON types, differ.
    Value {
        path: String,
        expected: Value,
        actual: Value,
    },
    /// Two byte strings (lowercase hex) differ; `offset` is the first differing byte.
    Bytes {
        path: String,
        offset: usize,
        expected_len: usize,
        actual_len: usize,
        expected_window: String,
        actual_window: String,
    },
    /// The oracle has a value that Heartwood does not.
    Missing { path: String, expected: Value },
    /// Heartwood has a value that the oracle does not.
    Extra { path: String, actual: Value },
    /// An object has the same keys on both sides, in another order.
    Order {
        path: String,
        expected: Vec<String>,
        actual: Vec<String>,
    },
}

impl Difference {
    pub fn path(&self) -> &str {
        match self {
            Difference::Value { path, .. }
            | Difference::Bytes { path, .. }
            | Difference::Missing { path, .. }
            | Difference::Extra { path, .. }
            | Difference::Order { path, .. } => path,
        }
    }
}

fn shown(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= MAX_SHOWN {
        text
    } else {
        let head: String = text.chars().take(MAX_SHOWN).collect();
        format!("{head}... ({} characters)", text.chars().count())
    }
}

impl fmt::Display for Difference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Difference::Value {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{path}: expected {}, actual {}",
                shown(expected),
                shown(actual)
            ),
            Difference::Bytes {
                path,
                offset,
                expected_len,
                actual_len,
                expected_window,
                actual_window,
            } => {
                write!(f, "{path}: bytes differ at offset {offset} (0x{offset:x})")?;
                if expected_len == actual_len {
                    write!(f, ", both {expected_len} bytes")?;
                } else {
                    write!(f, ", expected {expected_len} bytes, actual {actual_len}")?;
                }
                write!(
                    f,
                    "\n    expected {expected_window}\n    actual   {actual_window}"
                )
            }
            Difference::Missing { path, expected } => {
                write!(f, "{path}: missing, expected {}", shown(expected))
            }
            Difference::Extra { path, actual } => {
                write!(f, "{path}: not expected, actual {}", shown(actual))
            }
            Difference::Order {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{path}: keys in another order: expected {}, actual {}",
                shown(&Value::from(expected.clone())),
                shown(&Value::from(actual.clone()))
            ),
        }
    }
}

/// Compare `expected` with `actual` and list every difference, in document order.
pub fn diff(expected: &Value, actual: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    diff_at("$", expected, actual, &mut out);
    out
}

fn diff_at(path: &str, expected: &Value, actual: &Value, out: &mut Vec<Difference>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let same_keys = e.len() == a.len() && e.keys().all(|key| a.contains_key(key));
            let expected_order: Vec<&String> = e.keys().collect();
            let actual_order: Vec<&String> =
                js::property_order(a).into_iter().map(|(k, _)| k).collect();
            if same_keys && expected_order != actual_order {
                out.push(Difference::Order {
                    path: path.to_string(),
                    expected: expected_order.into_iter().cloned().collect(),
                    actual: actual_order.into_iter().cloned().collect(),
                });
            }
            for (key, ev) in e {
                let child = child_key(path, key);
                match a.get(key) {
                    Some(av) => diff_at(&child, ev, av, out),
                    None => out.push(Difference::Missing {
                        path: child,
                        expected: ev.clone(),
                    }),
                }
            }
            for (key, av) in a {
                if !e.contains_key(key) {
                    out.push(Difference::Extra {
                        path: child_key(path, key),
                        actual: av.clone(),
                    });
                }
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            for (i, ev) in e.iter().enumerate() {
                let child = format!("{path}[{i}]");
                match a.get(i) {
                    Some(av) => diff_at(&child, ev, av, out),
                    None => out.push(Difference::Missing {
                        path: child,
                        expected: ev.clone(),
                    }),
                }
            }
            for (i, av) in a.iter().enumerate().skip(e.len()) {
                out.push(Difference::Extra {
                    path: format!("{path}[{i}]"),
                    actual: av.clone(),
                });
            }
        }
        (Value::String(e), Value::String(a))
            if e != a && looks_like_bytes(e) && looks_like_bytes(a) =>
        {
            out.push(bytes_difference(path, e, a));
        }
        (Value::Number(e), Value::Number(a)) if js::number_value(e) == js::number_value(a) => {}
        _ if expected != actual => out.push(Difference::Value {
            path: path.to_string(),
            expected: expected.clone(),
            actual: actual.clone(),
        }),
        _ => {}
    }
}

fn child_key(path: &str, key: &str) -> String {
    let plain = !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if plain {
        format!("{path}.{key}")
    } else {
        format!("{path}[{}]", Value::String(key.to_string()))
    }
}

/// Lowercase hex of at least 8 bytes that is not just a decimal number: a letter a-f, or at least
/// 32 bytes (decimal amount strings are shorter than that).
fn looks_like_bytes(s: &str) -> bool {
    s.len() >= 16
        && s.len().is_multiple_of(2)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && (s.len() >= 64 || s.bytes().any(|b| b.is_ascii_alphabetic()))
}

fn bytes_difference(path: &str, expected: &str, actual: &str) -> Difference {
    let e = expected.as_bytes().chunks(2).collect::<Vec<_>>();
    let a = actual.as_bytes().chunks(2).collect::<Vec<_>>();
    let offset = e.iter().zip(&a).take_while(|(x, y)| x == y).count();
    Difference::Bytes {
        path: path.to_string(),
        offset,
        expected_len: e.len(),
        actual_len: a.len(),
        expected_window: window(&e, offset),
        actual_window: window(&a, offset),
    }
}

/// The bytes around `offset`, the byte at `offset` in brackets: `...0a1b[2c]3d...`.
fn window(bytes: &[&[u8]], offset: usize) -> String {
    let start = offset.saturating_sub(CONTEXT);
    let end = (offset + 1 + CONTEXT).min(bytes.len());
    let hex = |range: &[&[u8]]| -> String {
        range
            .iter()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .collect()
    };
    let mut out = String::new();
    if start > 0 {
        out.push_str("...");
    }
    out.push_str(&hex(&bytes[start..offset.min(bytes.len())]));
    if offset < bytes.len() {
        out.push('[');
        out.push_str(&hex(&bytes[offset..offset + 1]));
        out.push(']');
        out.push_str(&hex(&bytes[offset + 1..end]));
        if end < bytes.len() {
            out.push_str("...");
        }
    } else {
        out.push_str("[end]");
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn equal_values_have_no_difference() {
        let v = json!({"id": "ab", "json": {"fee": "100", "asset": {"votes": {"a": 50.5}}}, "list": [1, 2]});
        assert!(diff(&v, &v.clone()).is_empty());
    }

    #[test]
    fn scalar_differences_name_their_path() {
        let e = json!({"json": {"fee": "100000000", "nonce": "1"}, "isVerified": true});
        let a = json!({"json": {"fee": "10000000", "nonce": "1"}, "isVerified": false});
        let d = diff(&e, &a);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].path(), "$.json.fee");
        assert_eq!(
            d[1].to_string(),
            "$.isVerified: expected true, actual false"
        );
    }

    #[test]
    fn missing_and_extra_keys_and_elements() {
        let e = json!({"a": 1, "b": [1, 2, 3], "weird key": 0});
        let a = json!({"b": [1, 2], "c": 3, "weird key": 0});
        let d: Vec<String> = diff(&e, &a).iter().map(ToString::to_string).collect();
        assert_eq!(
            d,
            vec![
                "$.a: missing, expected 1",
                "$.b[2]: missing, expected 3",
                "$.c: not expected, actual 3",
            ]
        );
        let d = diff(&json!({"x y": 1}), &json!({"x y": 2}));
        assert_eq!(d[0].path(), "$[\"x y\"]");
    }

    #[test]
    fn hex_strings_get_a_byte_offset_and_a_window() {
        let e = "ff035a0100000006000100000000000000032b7179bebd249ce2";
        let a = "ff035a0100000006000100000000000000032b7179bebd249de2";
        let d = diff(&json!({"hex": e}), &json!({"hex": a}));
        assert_eq!(d.len(), 1);
        match &d[0] {
            Difference::Bytes {
                offset,
                expected_len,
                actual_len,
                expected_window,
                actual_window,
                ..
            } => {
                assert_eq!(*offset, 24);
                assert_eq!((*expected_len, *actual_len), (26, 26));
                assert_eq!(expected_window, "...00032b7179bebd24[9c]e2");
                assert_eq!(actual_window, "...00032b7179bebd24[9d]e2");
            }
            other => panic!("not a byte difference: {other:?}"),
        }
        assert!(
            d[0].to_string()
                .starts_with("$.hex: bytes differ at offset 24 (0x18), both 26 bytes")
        );
    }

    #[test]
    fn a_shorter_byte_string_ends_at_the_offset() {
        let e = "00112233445566778899aabbccddeeff";
        let a = "00112233445566778899aabb";
        let d = diff(&json!(e), &json!(a));
        match &d[0] {
            Difference::Bytes {
                offset,
                expected_window,
                actual_window,
                ..
            } => {
                assert_eq!(*offset, 12);
                assert_eq!(expected_window, "...445566778899aabb[cc]ddeeff");
                assert_eq!(actual_window, "...445566778899aabb[end]");
            }
            other => panic!("not a byte difference: {other:?}"),
        }
    }

    #[test]
    fn numbers_compare_by_their_double_value() {
        assert!(diff(&json!({"a": 8}), &json!({"a": 8.0})).is_empty());
        assert!(diff(&json!([0.5]), &json!([0.5])).is_empty());
        let d = diff(&json!({"a": 8}), &json!({"a": 8.5}));
        assert_eq!(d[0].to_string(), "$.a: expected 8, actual 8.5");
    }

    #[test]
    fn key_order_counts_in_javascript_enumeration_order() {
        let e: Value = serde_json::from_str(r#"{"b": 1, "a": 2}"#).unwrap();
        let a: Value = serde_json::from_str(r#"{"a": 2, "b": 1}"#).unwrap();
        let d = diff(&e, &a);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].to_string(),
            r#"$: keys in another order: expected ["b","a"], actual ["a","b"]"#
        );
        // Array-index keys enumerate first in JavaScript, whatever the insertion order.
        let e: Value = serde_json::from_str(r#"{"1": 1, "2": 2, "x": 3}"#).unwrap();
        let a: Value = serde_json::from_str(r#"{"x": 3, "2": 2, "1": 1}"#).unwrap();
        assert!(diff(&e, &a).is_empty());
    }

    #[test]
    fn decimal_amounts_are_not_bytes() {
        let d = diff(&json!("123456789012345678"), &json!("123456789012345679"));
        assert!(matches!(d[0], Difference::Value { .. }));
    }
}
