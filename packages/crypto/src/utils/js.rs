//! JavaScript number semantics, wherever the reference's numbers are observable.
//!
//! The reference reads JSON with `JSON.parse`, so every JSON number is an IEEE 754 double, and it
//! writes JSON with `JSON.stringify`, which prints numbers with `Number.prototype.toString`. This
//! module is the only place in the crate that models either, together with the two other parts of
//! JavaScript that decide how a parsed JSON value behaves: truthiness and the order in which an
//! object's keys are enumerated.

use serde_json::{Map, Number, Value};

/// The largest integer a double holds exactly, `2^53 − 1` (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The largest array index, `2^32 − 2`. Keys that spell an index are enumerated first.
const MAX_ARRAY_INDEX: u64 = 4_294_967_294;

/// The deepest indentation `JSON.stringify` uses: a larger `space` argument is treated as 10.
const MAX_INDENT: usize = 10;

/// The double that `JSON.parse` gives for the JSON number `number`.
///
/// For example `9007199254740993` gives `2^53`, and `8.0000000000000001` gives `8`.
pub fn number_value(number: &Number) -> f64 {
    // serde_json keeps an integer that fits 64 bits exactly and parses everything else as a
    // correctly rounded double. Converting the exact integer rounds to the nearest double, ties to
    // even, which is how `JSON.parse` reads the same digits.
    number.as_f64().unwrap_or(f64::NAN)
}

/// Whether `value` is an integer from `-(2^53 − 1)` to `2^53 − 1` (`Number.isSafeInteger`).
pub fn is_safe_integer(value: f64) -> bool {
    value.is_finite() && value.trunc() == value && value.abs() <= MAX_SAFE_INTEGER as f64
}

/// The integer in `value` if it is a JSON number whose double is a safe integer, as
/// `Number.isSafeInteger` decides after `JSON.parse`. `-0` gives 0.
pub fn safe_integer(value: &Value) -> Option<i64> {
    let Value::Number(number) = value else {
        return None;
    };
    let double = number_value(number);
    // A safe integer is at most 2^53 − 1 in magnitude, so the conversion is exact.
    is_safe_integer(double).then_some(double as i64)
}

/// Whether `value` is truthy: everything except `null`, `false`, `0`, `-0`, `NaN` and `""`.
/// Arrays and objects are truthy even when empty.
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => {
            let double = number_value(number);
            double != 0.0 && !double.is_nan()
        }
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `value` as `Number.prototype.toString` prints it.
///
/// Integers up to 21 digits print in full, so `100.0` prints as `100` and `1e20` as
/// `100000000000000000000`; larger and very small magnitudes use an exponent (`1e+21`, `1e-7`).
/// The digits are the shortest that read back as the same double.
pub fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == 0.0 {
        // Both zeros print as "0".
        return "0".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let (digits, n) = shortest_digits(value.abs());
    let k = digits.len() as i64;
    let mut out = String::from(sign);
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(n as usize);
        out.push_str(whole);
        out.push('.');
        out.push_str(fraction);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        let exponent = n - 1;
        let mut chars = digits.chars();
        if let Some(first) = chars.next() {
            out.push(first);
        }
        let rest = chars.as_str();
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push('e');
        out.push(if exponent < 0 { '-' } else { '+' });
        out.push_str(&exponent.unsigned_abs().to_string());
    }
    out
}

/// The shortest decimal digits `s` and the exponent `n` with `value = 0.s × 10^n`, for a finite
/// positive `value`, as `Number.prototype.toString` chooses them: the fewest digits that read back
/// as `value`, of those the closest to it, and of two equally close the one ending in an even
/// digit.
///
/// Rust's shortest scientific formatting gives the fewest and closest digits, but settles an exact
/// tie the other way, so a tie is detected and settled here.
fn shortest_digits(value: f64) -> (String, i64) {
    let (mut digits, n) = scientific_digits(&format!("{value:e}"));
    let Some(last) = digits.pop() else {
        return (digits, n);
    };
    let Some(last_digit) = last.to_digit(10) else {
        digits.push(last);
        return (digits, n);
    };
    if last_digit % 2 == 1 {
        // An odd last digit may be one side of a tie; the even neighbour on the other side of
        // `value` wins if it is exactly as close.
        for neighbour in [last_digit - 1, last_digit + 1] {
            let Some(neighbour_char) = char::from_digit(neighbour, 10) else {
                continue;
            };
            // Only a neighbour that also reads back as `value` can tie.
            let candidate: Option<f64> = format!("0.{digits}{neighbour_char}e{n}").parse().ok();
            if candidate != Some(value) {
                continue;
            }
            let low = char::from_digit(last_digit.min(neighbour), 10).unwrap_or('0');
            if is_exact_midpoint(value, &digits, low, n) {
                digits.push(neighbour_char);
                return (digits, n);
            }
        }
    }
    digits.push(last);
    (digits, n)
}

/// The digits and the exponent `n` (as in `0.digits × 10^n`) of Rust's scientific notation.
fn scientific_digits(scientific: &str) -> (String, i64) {
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    (digits, exponent + 1)
}

/// Whether `value` is exactly `0.{prefix}{low}5 × 10^n`: the midpoint between the candidates that
/// end in `low` and in `low + 1`.
fn is_exact_midpoint(value: f64, prefix: &str, low: char, n: i64) -> bool {
    // Every double has a finite decimal expansion of at most 767 significant digits, so 800
    // digits after the point print it exactly.
    let (exact, exact_n) = scientific_digits(&format!("{value:.800e}"));
    if exact_n != n {
        return false;
    }
    let Some(rest) = exact.strip_prefix(prefix) else {
        return false;
    };
    let mut rest = rest.chars();
    rest.next() == Some(low) && rest.next() == Some('5') && rest.all(|digit| digit == '0')
}

/// Whether `key` is an array index: the canonical decimal text of an integer from 0 to
/// `2^32 − 2`. JavaScript enumerates such keys first, in numeric order.
pub fn is_array_index(key: &str) -> bool {
    array_index(key).is_some()
}

fn array_index(key: &str) -> Option<u64> {
    let canonical = key == "0"
        || (!key.is_empty()
            && key.len() <= 10
            && !key.starts_with('0')
            && key.bytes().all(|byte| byte.is_ascii_digit()));
    if !canonical {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|index| *index <= MAX_ARRAY_INDEX)
}

/// The entries of `map` in the order JavaScript enumerates the keys of an object: array indexes
/// first, in numeric order, then the other keys in insertion order.
pub fn property_order(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut indexes: Vec<(u64, &String, &Value)> = Vec::new();
    let mut others: Vec<(&String, &Value)> = Vec::new();
    for (key, value) in map {
        match array_index(key) {
            Some(index) => indexes.push((index, key, value)),
            None => others.push((key, value)),
        }
    }
    indexes.sort_by_key(|(index, _, _)| *index);
    indexes
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(others)
        .collect()
}

/// `map` with its keys in the order JavaScript would enumerate them (see [`property_order`]).
pub(crate) fn in_property_order(map: Map<String, Value>) -> Map<String, Value> {
    if !map.keys().any(|key| is_array_index(key)) {
        return map;
    }
    let mut indexes: Vec<(u64, String, Value)> = Vec::new();
    let mut others: Vec<(String, Value)> = Vec::new();
    for (key, value) in map {
        match array_index(&key) {
            Some(index) => indexes.push((index, key, value)),
            None => others.push((key, value)),
        }
    }
    indexes.sort_by_key(|(index, _, _)| *index);
    indexes
        .into_iter()
        .map(|(_, key, value)| (key, value))
        .chain(others)
        .collect()
}

/// `value` as `JSON.stringify(value, null, indent)` writes it: `indent` spaces per level (at most
/// 10, as in JavaScript; 0 writes no whitespace at all), keys in the order JavaScript enumerates
/// them (see [`property_order`]), numbers as [`number_to_string`] prints them.
pub fn stringify(value: &Value, indent: usize) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent.min(MAX_INDENT), 0);
    out
}

fn write_value(out: &mut String, value: &Value, indent: usize, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            let double = number_value(number);
            if double.is_finite() {
                out.push_str(&number_to_string(double));
            } else {
                out.push_str("null");
            }
        }
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write_newline(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }
            write_newline(out, indent, depth);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (position, (key, item)) in property_order(map).into_iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write_newline(out, indent, depth + 1);
                write_string(out, key);
                out.push(':');
                if indent > 0 {
                    out.push(' ');
                }
                write_value(out, item, indent, depth + 1);
            }
            write_newline(out, indent, depth);
            out.push('}');
        }
    }
}

fn write_newline(out: &mut String, indent: usize, depth: usize) {
    if indent > 0 {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', indent * depth));
    }
}

/// A JSON string literal as `JSON.stringify` quotes it.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn parsed(text: &str) -> f64 {
        let value: Value = serde_json::from_str(text).unwrap();
        number_value(value.as_number().unwrap())
    }

    #[test]
    fn json_numbers_read_as_doubles() {
        assert_eq!(parsed("9007199254740993"), 9_007_199_254_740_992.0);
        assert_eq!(parsed("8.0000000000000001"), 8.0);
        assert_eq!(parsed("8e0"), 8.0);
        assert_eq!(parsed("2e6"), 2_000_000.0);
        assert!(parsed("-0").is_sign_negative());
        assert_eq!(parsed("-0"), 0.0);
        assert_eq!(parsed("18446744073709551616"), 18_446_744_073_709_551_616.0);
        assert_eq!(parsed("-9223372036854775809"), -9_223_372_036_854_775_808.0);
    }

    #[test]
    fn safe_integers() {
        assert!(is_safe_integer(0.0));
        assert!(is_safe_integer(-0.0));
        assert!(is_safe_integer(MAX_SAFE_INTEGER as f64));
        assert!(is_safe_integer(-(MAX_SAFE_INTEGER as f64)));
        assert!(!is_safe_integer(9_007_199_254_740_992.0));
        assert!(!is_safe_integer(1.5));
        assert!(!is_safe_integer(f64::NAN));
        assert!(!is_safe_integer(f64::INFINITY));
        assert_eq!(safe_integer(&json!(-0.0)), Some(0));
        assert_eq!(safe_integer(&json!(8.0)), Some(8));
        assert_eq!(safe_integer(&json!("8")), None);
        assert_eq!(safe_integer(&json!(9_007_199_254_740_993_u64)), None);
    }

    #[test]
    fn truthiness() {
        for value in [json!(null), json!(false), json!(0), json!(-0.0), json!("")] {
            assert!(!is_truthy(&value), "{value}");
        }
        for value in [
            json!(true),
            json!(1),
            json!(-1.5),
            json!("0"),
            json!([]),
            json!({}),
        ] {
            assert!(is_truthy(&value), "{value}");
        }
    }

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.0, "-1"),
            (100.0, "100"),
            (1.89, "1.89"),
            (33.33, "33.33"),
            (0.1, "0.1"),
            (0.000001, "0.000001"),
            (0.0000015, "0.0000015"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1.25e21, "1.25e+21"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (9_007_199_254_740_992.0, "9007199254740992"),
            (5e-324, "5e-324"),
            // Exact ties between two shortest candidates go to the even digit.
            (2_056_935_690_371_866.0 + 0.25, "2056935690371866.2"),
            (-181_396_803_010_236.0 - 0.125, "-181396803010236.12"),
            (26_218_211_955_751.0 + 0.0625, "26218211955751.062"),
            (f64::MAX, "1.7976931348623157e+308"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ];
        for (value, text) in cases {
            assert_eq!(number_to_string(*value), *text, "{value:e}");
        }
    }

    #[test]
    fn array_indexes() {
        assert!(is_array_index("0"));
        assert!(is_array_index("53"));
        assert!(is_array_index("4294967294"));
        assert!(!is_array_index("4294967295"));
        assert!(!is_array_index("01"));
        assert!(!is_array_index("-1"));
        assert!(!is_array_index("1.0"));
        assert!(!is_array_index(""));
        assert!(!is_array_index("a1"));
    }

    #[test]
    fn stringify_matches_json_stringify() {
        let value: Value = serde_json::from_str(
            r#"{"b":[1,2.5,{}],"10":true,"a":{"x":null},"2":"q\"\n\u0001é","e":[]}"#,
        )
        .unwrap();
        assert_eq!(
            stringify(&value, 0),
            r#"{"2":"q\"\n\u0001é","10":true,"b":[1,2.5,{}],"a":{"x":null},"e":[]}"#
        );
        assert_eq!(
            stringify(&value, 4),
            "{\n    \"2\": \"q\\\"\\n\\u0001é\",\n    \"10\": true,\n    \"b\": [\n        1,\n        \
             2.5,\n        {}\n    ],\n    \"a\": {\n        \"x\": null\n    },\n    \"e\": []\n}"
        );
        assert_eq!(stringify(&json!(8.0), 2), "8");
        assert_eq!(stringify(&json!([1]), 12), stringify(&json!([1]), 10));
    }

    #[test]
    fn property_order_puts_indexes_first() {
        let map: Map<String, Value> =
            serde_json::from_str(r#"{"b":1,"10":2,"a":3,"2":4,"01":5}"#).unwrap();
        let keys: Vec<&str> = property_order(&map)
            .into_iter()
            .map(|(key, _)| key.as_str())
            .collect();
        assert_eq!(keys, ["2", "10", "b", "a", "01"]);
        let ordered = in_property_order(map);
        let keys: Vec<&str> = ordered.keys().map(String::as_str).collect();
        assert_eq!(keys, ["2", "10", "b", "a", "01"]);
    }

    proptest! {
        #[test]
        fn printed_numbers_read_back(bits in any::<u64>()) {
            let value = f64::from_bits(bits);
            prop_assume!(value.is_finite());
            let text = number_to_string(value);
            let back: f64 = text.parse().unwrap();
            prop_assert_eq!(back, value);
            prop_assert!(back.to_bits() == value.to_bits() || value == 0.0);
        }

        #[test]
        fn safe_integers_print_as_integers(value in -(MAX_SAFE_INTEGER as i64)..=MAX_SAFE_INTEGER as i64) {
            prop_assert_eq!(number_to_string(value as f64), value.to_string());
        }
    }
}
