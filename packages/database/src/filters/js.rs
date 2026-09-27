//! JavaScript's `Number` of a string.

/// `text` as JavaScript's `Number(text)` reads it: surrounding white space is ignored; the empty
/// string is 0; `0x`, `0o` and `0b` prefix hexadecimal, octal and binary digits (without a sign);
/// `Infinity` may have a sign; otherwise a decimal literal with an optional sign, fraction and
/// exponent. Anything else is `NaN`.
pub fn js_number(text: &str) -> f64 {
    let text = text.trim_matches(is_js_space);
    if text.is_empty() {
        return 0.0;
    }
    for (prefixes, radix) in [(["0x", "0X"], 16), (["0o", "0O"], 8), (["0b", "0B"], 2)] {
        if let Some(digits) = prefixes.iter().find_map(|prefix| text.strip_prefix(prefix)) {
            return in_radix(digits, radix);
        }
    }
    let (sign, body) = match text.as_bytes().first() {
        Some(b'-') => (-1.0, text.get(1..).unwrap_or_default()),
        Some(b'+') => (1.0, text.get(1..).unwrap_or_default()),
        _ => (1.0, text),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal(body) {
        return f64::NAN;
    }
    // Rust reads a decimal literal to the nearest double, as JavaScript does.
    body.parse::<f64>().map_or(f64::NAN, |value| sign * value)
}

/// Whether `c` is white space or a line terminator to JavaScript.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// The value of `digits` in `radix`; `NaN` when there is none or one is not a digit.
fn in_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut value = 0.0_f64;
    for c in digits.chars() {
        let Some(digit) = c.to_digit(radix) else {
            return f64::NAN;
        };
        value = value * f64::from(radix) + f64::from(digit);
    }
    value
}

/// Whether `text` is a decimal literal without a sign: digits with an optional fraction, or a
/// fraction alone, then an optional exponent.
fn is_decimal(text: &str) -> bool {
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes.get(from..).map_or(0, |rest| {
            rest.iter().take_while(|b| b.is_ascii_digit()).count()
        })
    };
    let whole = digits(0);
    let mut end = whole;
    let mut fraction = 0;
    if bytes.get(end) == Some(&b'.') {
        fraction = digits(end + 1);
        end += 1 + fraction;
    }
    if whole == 0 && fraction == 0 {
        return false;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        end += 1;
        if matches!(bytes.get(end), Some(b'+' | b'-')) {
            end += 1;
        }
        let exponent = digits(end);
        if exponent == 0 {
            return false;
        }
        end += exponent;
    }
    end == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_literals_read_as_javascript_reads_them() {
        let cases = [
            ("5", 5.0),
            ("05", 5.0),
            ("+5", 5.0),
            ("-5", -5.0),
            ("5.", 5.0),
            (".5", 0.5),
            ("1e3", 1000.0),
            ("1E+3", 1000.0),
            ("25e-1", 2.5),
            (" \t5\n", 5.0),
            ("\u{feff}7\u{a0}", 7.0),
            ("", 0.0),
            ("   ", 0.0),
            ("0x10", 16.0),
            ("0X1f", 31.0),
            ("0o17", 15.0),
            ("0b101", 5.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("1e999", f64::INFINITY),
        ];
        for (text, expected) in cases {
            assert_eq!(js_number(text), expected, "{text:?}");
        }
    }

    #[test]
    fn other_text_is_not_a_number() {
        for text in [
            "abc", "5a", "1_000", ".", "e5", "1e", "1e+", "--5", "+-5", "-0x10", "0x", "0xg",
            "0b2", "inf", "nan", "NaN", "infinity", "5 5", "\u{85}5",
        ] {
            assert!(js_number(text).is_nan(), "{text:?}");
        }
    }
}
