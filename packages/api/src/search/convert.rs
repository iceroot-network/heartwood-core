//! JavaScript's conversions that the standard criteria rely on: `Number(x)`, `String(x)` and
//! `BigInt(x)` of a JSON value, with an unbounded integer ([`Big`]) for the BigNumbers.

use std::cmp::Ordering;

use heartwood_crypto::utils::js;
use serde_json::Value;

/// JavaScript's white space and line terminators (`\s` and `String.prototype.trim`).
pub(crate) fn is_space(c: char) -> bool {
    "\t\n\u{b}\u{c}\r \u{a0}\u{1680}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}".contains(c)
        || ('\u{2000}'..='\u{200a}').contains(&c)
}

/// `String(value)`: an array joins its items with commas (`null` as nothing), an object is
/// `[object Object]`.
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js::number_to_string(js::number_value(number)),
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let text = |item: &Value| {
                if item.is_null() {
                    String::new()
                } else {
                    text(item)
                }
            };
            items.iter().map(text).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Number(value)`.
pub(crate) fn number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Number(number) => js::number_value(number),
        Value::String(text) => parse_number(text),
        Value::Array(_) => parse_number(&text(value)),
        Value::Object(_) => f64::NAN,
    }
}

/// `Number(text)`: white space around it is ignored, empty is 0, `0x`, `0o` and `0b` digits,
/// `Infinity`, and decimal literals; anything else is NaN.
fn parse_number(text: &str) -> f64 {
    let text = text.trim_matches(is_space);
    if text.is_empty() {
        return 0.0;
    }
    if let Some((radix, digits)) = radix_prefix(text) {
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        let digit = |c: char| f64::from(c.to_digit(radix).unwrap_or(0));
        return digits
            .chars()
            .fold(0.0, |sum, c| sum * f64::from(radix) + digit(c));
    }
    let (sign, body) = match text.strip_prefix('-') {
        Some(body) => (-1.0, body),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent = exponent.map(|e| e.strip_prefix(['+', '-']).unwrap_or(e));
    let exponent_ok = exponent.is_none_or(|e| !e.is_empty() && digits(e));
    match body.parse::<f64>() {
        Ok(value) if mantissa_ok && exponent_ok => sign * value,
        _ => f64::NAN,
    }
}

/// The radix and digits of a `0x`, `0o` or `0b` literal.
fn radix_prefix(text: &str) -> Option<(u32, &str)> {
    let prefix = text.get(..2)?.to_ascii_lowercase();
    let radix = match prefix.as_str() {
        "0x" => 16,
        "0o" => 8,
        "0b" => 2,
        _ => return None,
    };
    Some((radix, &text[2..]))
}

/// An integer of any size: `BigInt`'s value, as the sign and the decimal digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Big {
    negative: bool,
    /// Decimal digits, most significant first, without leading zeros (none for zero).
    digits: Vec<u8>,
}

impl Big {
    fn new(negative: bool, mut digits: Vec<u8>) -> Big {
        let zeros = digits.iter().take_while(|d| **d == 0).count();
        digits.drain(..zeros);
        Big {
            negative: negative && !digits.is_empty(),
            digits,
        }
    }

    /// `BigInt(text)`: decimal with an optional sign, or `0x`, `0o`, `0b` digits; empty is 0.
    pub(crate) fn parse(text: &str) -> Option<Big> {
        let text = text.trim_matches(is_space);
        if let Some((radix, digits)) = radix_prefix(text) {
            let mut out = Vec::new();
            for digit in digits.chars() {
                mul_add(&mut out, radix, digit.to_digit(radix)?);
            }
            return (!digits.is_empty()).then(|| Big::new(false, out));
        }
        let (negative, body) = match text.strip_prefix('-') {
            Some(body) => (true, body),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let digits = body.bytes().all(|b| b.is_ascii_digit());
        let empty = text.is_empty();
        (digits && (empty || !body.is_empty()))
            .then(|| Big::new(negative, body.bytes().map(|b| b - b'0').collect()))
    }

    /// `BigInt(value)`: text, an integer number, a boolean, or an array through its text; an
    /// object, `null` and a fraction give `None` (`BigInt` throws).
    pub(crate) fn of(value: &Value) -> Option<Big> {
        match value {
            Value::String(text) => Big::parse(text),
            Value::Bool(flag) => Some(Big::new(false, vec![u8::from(*flag)])),
            Value::Number(n) => {
                let n = js::number_value(n);
                (n.is_finite() && n.fract() == 0.0)
                    .then(|| Big::parse(&format!("{n:.0}")))
                    .flatten()
            }
            Value::Array(_) => Big::parse(&text(value)),
            Value::Null | Value::Object(_) => None,
        }
    }

    /// The product of two non-negative integers.
    pub(crate) fn times(&self, other: &Big) -> Big {
        let mut out: Vec<u32> = vec![0; self.digits.len() + other.digits.len()];
        for (i, a) in self.digits.iter().rev().enumerate() {
            for (j, b) in other.digits.iter().rev().enumerate() {
                if let Some(cell) = out.get_mut(i + j) {
                    *cell += u32::from(*a) * u32::from(*b);
                }
            }
        }
        let mut carry = 0;
        for cell in &mut out {
            let total = *cell + carry;
            (*cell, carry) = (total % 10, total / 10);
        }
        Big::new(false, out.iter().rev().map(|d| *d as u8).collect())
    }

    /// The quotient of two non-negative integers, truncated; `None` when `other` is zero.
    pub(crate) fn divided_by(&self, other: &Big) -> Option<Big> {
        if other.digits.is_empty() {
            return None;
        }
        let (mut rest, mut quotient) = (Big::new(false, Vec::new()), Vec::new());
        for digit in &self.digits {
            rest.digits.push(*digit);
            rest = Big::new(false, rest.digits);
            let mut count = 0;
            while rest >= *other {
                rest = rest.minus(other);
                count += 1;
            }
            quotient.push(count);
        }
        Some(Big::new(false, quotient))
    }

    /// `self - other` for `other <= self`, both non-negative.
    fn minus(&self, other: &Big) -> Big {
        let mut out = self.digits.clone();
        let mut borrow = 0_i8;
        for (i, cell) in out.iter_mut().rev().enumerate() {
            let sub = other.digits.iter().rev().nth(i).map_or(0, |d| *d as i8) + borrow;
            let value = *cell as i8 - sub;
            (*cell, borrow) = if value < 0 {
                ((value + 10) as u8, 1)
            } else {
                (value as u8, 0)
            };
        }
        Big::new(false, out)
    }

    /// The value as a double, rounded to nearest as JavaScript's `Number(string)` rounds.
    pub(crate) fn to_f64(&self) -> f64 {
        self.to_string().parse().unwrap_or(f64::NAN)
    }
}

/// `digits * base + add`, in decimal.
fn mul_add(digits: &mut Vec<u8>, base: u32, add: u32) {
    let mut carry = add;
    for digit in digits.iter_mut().rev() {
        let value = u32::from(*digit) * base + carry;
        (*digit, carry) = ((value % 10) as u8, value / 10);
    }
    while carry > 0 {
        digits.insert(0, (carry % 10) as u8);
        carry /= 10;
    }
}

impl std::fmt::Display for Big {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digits: String = self.digits.iter().map(|d| char::from(b'0' + d)).collect();
        match (digits.is_empty(), self.negative) {
            (true, _) => f.write_str("0"),
            (false, true) => write!(f, "-{digits}"),
            (false, false) => f.write_str(&digits),
        }
    }
}

impl Ord for Big {
    fn cmp(&self, other: &Big) -> Ordering {
        let size = self
            .digits
            .len()
            .cmp(&other.digits.len())
            .then_with(|| self.digits.cmp(&other.digits));
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (true, true) => size.reverse(),
            (false, false) => size,
        }
    }
}

impl PartialOrd for Big {
    fn partial_cmp(&self, other: &Big) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn big(text: &str) -> String {
        Big::parse(text).map_or_else(|| "throws".into(), |big| big.to_string())
    }

    #[test]
    fn numbers_read_as_javascript_reads_them() {
        for (text, expected) in [
            ("", 0.0),
            ("  12  ", 12.0),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("-0x10", f64::NAN),
            ("0x10", 16.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("+7", 7.0),
            ("-Infinity", f64::NEG_INFINITY),
            ("inf", f64::NAN),
            ("nan", f64::NAN),
            ("1e", f64::NAN),
            ("0x", f64::NAN),
            ("1 2", f64::NAN),
            (".", f64::NAN),
        ] {
            let got = number(&json!(text));
            assert!(
                got == expected || (got.is_nan() && expected.is_nan()),
                "{text:?}: {got}"
            );
        }
        assert_eq!(number(&json!([5])), 5.0);
        assert_eq!(number(&json!([])), 0.0);
        assert!(number(&json!([1, 2])).is_nan());
        assert!(number(&json!({})).is_nan());
        assert_eq!(number(&json!(null)), 0.0);
        assert_eq!(number(&json!(true)), 1.0);
    }

    #[test]
    fn text_joins_arrays() {
        assert_eq!(text(&json!([1, null, "a", [2, 3]])), "1,,a,2,3");
        assert_eq!(text(&json!({})), "[object Object]");
        assert_eq!(text(&json!(1.5)), "1.5");
    }

    #[test]
    fn big_integers_read_as_bigint_reads_them() {
        assert_eq!(big(""), "0");
        assert_eq!(big(" -12 "), "-12");
        assert_eq!(big("+0012"), "12");
        assert_eq!(big("-0"), "0");
        assert_eq!(big("0x10"), "16");
        assert_eq!(big("0xffffffffffffffffffff"), "1208925819614629174706175");
        for text in ["1.5", "1e3", "-", "0x", "-0x10", "1_0", "abc"] {
            assert_eq!(big(text), "throws", "{text}");
        }
        assert_eq!(
            Big::of(&json!(1e21)).unwrap().to_string(),
            "1000000000000000000000"
        );
        assert!(Big::of(&json!(1.5)).is_none());
        assert!(Big::of(&json!(null)).is_none());
        assert_eq!(Big::of(&json!([7])).unwrap().to_string(), "7");
    }

    #[test]
    fn big_integers_multiply_divide_and_compare() {
        let parse = |text: &str| Big::parse(text).unwrap();
        let product =
            parse("123456789012345678901234567890").times(&parse("987654321098765432109876543210"));
        assert_eq!(
            product.to_string(),
            "121932631137021795226185032733622923332237463801111263526900"
        );
        assert_eq!(
            product.divided_by(&parse("987654321098765432109876543210")),
            Some(parse("123456789012345678901234567890"))
        );
        assert_eq!(parse("10").divided_by(&parse("3")), Some(parse("3")));
        assert_eq!(parse("2").divided_by(&parse("3")), Some(parse("0")));
        assert_eq!(parse("2").divided_by(&parse("0")), None);
        assert!(parse("-5") < parse("3") && parse("-5") < parse("-3") && parse("10") > parse("9"));
        assert_eq!(parse("1000").to_f64(), 1000.0);
    }
}
