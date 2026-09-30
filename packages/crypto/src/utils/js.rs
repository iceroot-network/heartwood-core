//! JavaScript number semantics, wherever the reference's numbers are observable.
//!
//! The reference reads JSON with `JSON.parse`, so every JSON number is an IEEE 754 double, and it
//! writes JSON with `JSON.stringify`, which prints numbers with `Number.prototype.toString`. This
//! module is the only place in the crate that models either, together with the two other parts of
//! JavaScript that decide how a parsed JSON value behaves: truthiness and the order in which an
//! object's keys are enumerated. It also writes numbers as `toLocaleString` does in English,
//! which the reference's messages use for amounts and counts, reads text as `BigInt(text)` and
//! `Number(text) === 0` do, and quotes a string as `JSON.stringify` does.

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
        Value::String(text) => json_string(out, text),
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
                json_string(out, key);
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

/// Append `JSON.stringify(text)` to `out`: the text in double quotes, with `"` and `\` escaped,
/// U+0008, U+0009, U+000A, U+000C and U+000D as `\b`, `\t`, `\n`, `\f` and `\r`, and the other
/// code points below U+0020 as `\u00xx` in lowercase hex. Everything else is written as it is.
pub fn json_string(out: &mut String, text: &str) {
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

/// Why `BigInt(text)` gives no `i128`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigIntError {
    /// The text is not a `StringIntegerLiteral`: `BigInt` throws a `SyntaxError`.
    Syntax,
    /// The text is an integer literal whose value needs more than 128 bits.
    TooWide,
}

/// `BigInt(text)` for a string, as an `i128`.
///
/// White space and line terminators around the literal are ignored, and a text of white space
/// only is 0. The literal is a decimal integer with an optional sign, or `0x`, `0o` or `0b` (either
/// case) with at least one digit of that base and no sign. Anything else, including a fraction or
/// an exponent, is a syntax error. The whole text is checked before its value is computed, so a
/// syntax error wins over a value that is too wide.
pub fn parse_bigint(text: &str) -> Result<i128, BigIntError> {
    let literal = text.trim_matches(is_js_space);
    if literal.is_empty() {
        return Ok(0);
    }
    let prefixed = |upper: &str, lower: &str| {
        literal
            .strip_prefix(upper)
            .or_else(|| literal.strip_prefix(lower))
    };
    let (negative, radix, digits) = if let Some(digits) = prefixed("0x", "0X") {
        (false, 16, digits)
    } else if let Some(digits) = prefixed("0o", "0O") {
        (false, 8, digits)
    } else if let Some(digits) = prefixed("0b", "0B") {
        (false, 2, digits)
    } else if let Some(digits) = literal.strip_prefix('-') {
        (true, 10, digits)
    } else {
        (false, 10, literal.strip_prefix('+').unwrap_or(literal))
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(BigIntError::Syntax);
    }
    let mut magnitude: u128 = 0;
    for c in digits.chars() {
        let digit = u128::from(c.to_digit(radix).ok_or(BigIntError::Syntax)?);
        magnitude = magnitude
            .checked_mul(u128::from(radix))
            .and_then(|m| m.checked_add(digit))
            .ok_or(BigIntError::TooWide)?;
    }
    if negative {
        0i128.checked_sub_unsigned(magnitude)
    } else {
        i128::try_from(magnitude).ok()
    }
    .ok_or(BigIntError::TooWide)
}

/// `Number(text) === 0` for a string: empty or white space, a zero in any base, or a decimal
/// literal whose double value is 0 (`-0` and an underflow such as `1e-400` included).
pub fn reads_as_zero(text: &str) -> bool {
    let text = text.trim_matches(is_js_space);
    parse_bigint(text) == Ok(0)
        || (text
            .bytes()
            .all(|b| b.is_ascii_digit() || b".+-eE".contains(&b))
            && text.parse::<f64>() == Ok(0.0))
}

/// JavaScript's white space and line terminators (`StrWhiteSpaceChar`): tab, vertical tab, form
/// feed, the byte-order mark, the space separators (Unicode category Zs), LF, CR, U+2028 and
/// U+2029. Unlike [`char::is_whitespace`], U+0085 is not one of them and U+FEFF is.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `value.toLocaleString("en", {minimumFractionDigits: 0, maximumFractionDigits})`, as Node's ICU
/// writes it.
///
/// ICU rounds the shortest decimal digits of the double, the digits `Number.prototype.toString`
/// prints, not its exact binary value: `1.000000005` (exactly `1.00000000499999996…`) is
/// `"1.00000001"` at 8 digits, and `2^64` is `"18,446,744,073,709,552,000"`. The digits are
/// rounded to `max_fraction_digits` fraction digits with a tie away from zero, trailing fraction
/// zeros are removed, and the integer part is grouped by three with commas. A negative value,
/// `-0` included, keeps its `-` even when it rounds to 0; `NaN` is `"NaN"` and an infinity `"∞"`.
pub fn to_locale_fixed(value: f64, max_fraction_digits: u8) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value.is_infinite() {
        return format!("{sign}∞");
    }
    let fraction = usize::from(max_fraction_digits);
    let (digits, n) = match value {
        0.0 => (String::new(), 0),
        _ => shortest_digits(value.abs()),
    };
    // `value × 10^fraction` is `0.digits × 10^keep`; its integer part is the first `keep` digits.
    let keep = usize::try_from(n.saturating_add(i64::from(max_fraction_digits))).ok();
    let zeros = std::iter::repeat(b'0');
    let mut scaled: Vec<u8> = digits
        .bytes()
        .chain(zeros)
        .take(keep.unwrap_or(0))
        .collect();
    let first_dropped = keep.and_then(|keep| digits.as_bytes().get(keep)).copied();
    if first_dropped.is_some_and(|digit| digit >= b'5') {
        let carried = scaled.iter_mut().rev().all(|digit| {
            let nine = *digit == b'9';
            *digit = if nine { b'0' } else { *digit + 1 };
            nine
        });
        if carried {
            scaled.insert(0, b'1');
        }
    }
    let (whole, part) = scaled.split_at(scaled.len().saturating_sub(fraction));
    let mut part = format!("{:0>fraction$}", String::from_utf8_lossy(part));
    part.truncate(part.trim_end_matches('0').len());
    let whole = String::from_utf8_lossy(if whole.is_empty() { b"0" } else { whole });
    let mut out = String::from(sign);
    for (position, digit) in whole.chars().enumerate() {
        if position > 0 && (whole.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if !part.is_empty() {
        out.push('.');
        out.push_str(&part);
    }
    out
}

/// An integer's `toLocaleString()` in English, as a JavaScript number holds it: the nearest
/// double, written by [`to_locale_fixed`] (digits grouped by three with commas, a leading `-` for
/// a negative value).
pub fn to_locale_integer(value: i128) -> String {
    // The conversion rounds to the nearest double, a tie to even, as `Number` does.
    to_locale_fixed(value as f64, 0)
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
    fn json_strings_are_escaped_as_json_stringify_does() {
        let mut out = String::new();
        json_string(
            &mut out,
            "a\"\\\u{08}\t\n\u{0B}\u{0C}\r\u{1F}\u{7F}\u{2028}\u{e9}",
        );
        assert_eq!(
            out,
            "\"a\\\"\\\\\\b\\t\\n\\u000b\\f\\r\\u001f\u{7F}\u{2028}\u{e9}\""
        );
    }

    #[test]
    fn bigint_reads_text_as_javascript_does() {
        let ok = |text: &str| parse_bigint(text);
        assert_eq!(ok(""), Ok(0));
        assert_eq!(ok(" \t\n\u{FEFF}\u{3000}"), Ok(0));
        assert_eq!(ok(" 12\n"), Ok(12));
        assert_eq!(ok("-0"), Ok(0));
        assert_eq!(ok("+5"), Ok(5));
        assert_eq!(ok("-5"), Ok(-5));
        assert_eq!(ok("0x10"), Ok(16));
        assert_eq!(ok("0XfF"), Ok(255));
        assert_eq!(ok("0o17"), Ok(15));
        assert_eq!(ok("0b101"), Ok(5));
        assert_eq!(ok("007"), Ok(7));
        assert_eq!(
            ok("-170141183460469231731687303715884105728"),
            Ok(i128::MIN)
        );
        assert_eq!(ok("170141183460469231731687303715884105727"), Ok(i128::MAX));
        for bad in [
            "1.5", "1e3", "-0x10", "+0x1", "0x", "0b2", "--1", "+", "-", "1 2", "1_000", "\u{85}1",
            "Infinity", "0n",
        ] {
            assert_eq!(ok(bad), Err(BigIntError::Syntax), "{bad:?}");
        }
        assert_eq!(
            ok("170141183460469231731687303715884105728"),
            Err(BigIntError::TooWide)
        );
        assert_eq!(
            ok(&format!("0x{}", "f".repeat(40))),
            Err(BigIntError::TooWide)
        );
        // A syntax error later in the text wins over the width.
        assert_eq!(
            ok(&format!("{}x", "9".repeat(60))),
            Err(BigIntError::Syntax)
        );
    }

    #[test]
    fn texts_that_read_as_the_number_zero() {
        for zero in [
            "",
            " \t\n\u{0B}\u{0C}\r",
            "0",
            "00",
            "-0",
            "+0",
            "0.",
            ".0",
            "+.0",
            "-.0e-0",
            "0e5",
            "0.0e+999999999999",
            "1e-400",
            "0x0",
            "0X00",
            "0o0",
            "0b0",
            " 0 ",
            "\u{A0}0\u{FEFF}",
            "\u{2028}0\u{2029}",
            &format!("0x{}", "0".repeat(60)),
        ] {
            assert!(reads_as_zero(zero), "{zero:?}");
        }
        for other in [
            "1",
            "1e-300",
            "5e-324",
            "x",
            "-0x0",
            "+0x0",
            "0x",
            "0n",
            "0e",
            ".",
            "e0",
            "0_0",
            "0\u{85}",
            "\u{180E}0",
            "Infinity",
            "NaN",
        ] {
            assert!(!reads_as_zero(other), "{other:?}");
        }
    }

    #[test]
    fn javascript_white_space_is_unicode_white_space_without_u0085_and_with_the_bom() {
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let unicode = (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}';
            assert_eq!(is_js_space(c), unicode, "U+{:04X}", u32::from(c));
        }
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

    /// Answers of Node 18 (ICU 74) to `x.toLocaleString("en", {minimumFractionDigits: 0,
    /// maximumFractionDigits: 8})`.
    #[test]
    fn locale_fixed_rounds_the_shortest_digits_half_up() {
        let cases: [(f64, &str); 23] = [
            (1.000000005, "1.00000001"),
            (1.000000015, "1.00000002"),
            (1.0000000249999998, "1.00000002"),
            (1.000000035, "1.00000004"),
            (5e-9, "0.00000001"),
            (1.5e-8, "0.00000002"),
            (3.4999999999999996e-8, "0.00000003"),
            (9.5e-8, "0.0000001"),
            (1.0499999999999999e-7, "0.0000001"),
            (-0.0, "-0"),
            (0.0, "0"),
            (-1e-10, "-0"),
            (1e-10, "0"),
            (-1e-8, "-0.00000001"),
            (1e21, "1,000,000,000,000,000,000,000"),
            (
                1.2345678901234568e29,
                "123,456,789,012,345,680,000,000,000,000",
            ),
            (18_446_744_073_709_551_616.0, "18,446,744,073,709,552,000"),
            (9_007_199_254_740_994.0, "9,007,199,254,740,994"),
            (0.30000000000000004, "0.3"),
            (0.999999995, "1"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "∞"),
            (f64::NEG_INFINITY, "-∞"),
        ];
        for (value, text) in cases {
            assert_eq!(to_locale_fixed(value, 8), text, "{value:e}");
        }
        let rounded_up: f64 = "99999999.999999999".parse().unwrap();
        assert_eq!(to_locale_fixed(rounded_up, 8), "100,000,000");
        assert_eq!(
            to_locale_fixed(99_999_999.999_999_99, 8),
            "99,999,999.99999999"
        );
        assert_eq!(to_locale_fixed(1234.5, 3), "1,234.5");
        assert_eq!(to_locale_fixed(0.5, 0), "1");
        assert_eq!(to_locale_fixed(-2.5, 0), "-3");
        assert_eq!(to_locale_integer(2_000_000), "2,000,000");
        assert_eq!(to_locale_integer(-1234), "-1,234");
        assert_eq!(to_locale_integer(999), "999");
        assert_eq!(
            to_locale_integer(12_345_678_901_234_567_890),
            "12,345,678,901,234,567,000"
        );
    }

    proptest! {
        #[test]
        fn locale_integers_below_two_to_the_53_are_exact(value in -(MAX_SAFE_INTEGER as i64)..=MAX_SAFE_INTEGER as i64) {
            let text = to_locale_integer(i128::from(value));
            prop_assert_eq!(text.replace(',', ""), value.to_string());
            let digits = text.trim_start_matches('-');
            prop_assert!(digits.split(',').skip(1).all(|group| group.len() == 3));
        }

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
