//! The parts of JavaScript that decide the reference's bytes and values on the wire: the own-key
//! order of a plain object, how `BigInt` reads text, UTF-16 lengths, order and cuts, Node's lossy
//! UTF-8 decoding, and how `JSON.stringify` escapes a string.

use std::cmp::Ordering;

use heartwood_crypto::utils::js::is_array_index;

#[cfg(test)]
mod tests;

/// The key whose assignment replaces a plain object's prototype instead of adding an own key.
pub(crate) const PROTO_KEY: &str = "__proto__";

/// What the assignment `object[key] = value` did to a plain object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Assigned {
    /// A new own key.
    Inserted,
    /// An existing own key took the value and kept its position.
    Replaced,
    /// The key was `__proto__`: the object's prototype was replaced and no own key was added.
    Prototype,
}

/// `object[key] = value` on a plain object whose own keys are `entries`, in enumeration order.
///
/// An existing key keeps its position and takes the value. A new array-index key goes among the
/// array-index keys, which come first, in numeric order; any other new key goes last. The key
/// `__proto__` adds nothing (the reference's object takes the value as its prototype).
pub(crate) fn assign<V>(entries: &mut Vec<(String, V)>, key: String, value: V) -> Assigned {
    if key == PROTO_KEY {
        return Assigned::Prototype;
    }
    if let Some(entry) = entries.iter_mut().find(|(existing, _)| *existing == key) {
        entry.1 = value;
        return Assigned::Replaced;
    }
    let at = match array_index(&key) {
        Some(index) => entries
            .iter()
            .position(|(existing, _)| array_index(existing).is_none_or(|other| other > index))
            .unwrap_or(entries.len()),
        None => entries.len(),
    };
    entries.insert(at, (key, value));
    Assigned::Inserted
}

/// The numeric value of an array-index key.
fn array_index(key: &str) -> Option<u64> {
    if is_array_index(key) {
        key.parse().ok()
    } else {
        None
    }
}

/// Why `BigInt(text)` gives no `i128`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BigIntError {
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
pub(crate) fn parse_bigint(text: &str) -> Result<i128, BigIntError> {
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

/// The text of `bytes` as Node's `Buffer.toString()` decodes it: each maximal invalid UTF-8
/// subpart becomes U+FFFD.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the frame and protobuf readers call it")
)]
pub(crate) fn utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The length of `text` in UTF-16 code units, JavaScript's `text.length`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the frame writer and the reply check call it")
)]
pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The order of JavaScript's default `sort()` on strings: by UTF-16 code units.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the canonical JSON writer calls it")
)]
pub(crate) fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `text.slice(0, at)` and `text.slice(at)` in UTF-16 code units, as `Buffer.from` then writes
/// them: a surrogate pair cut in half becomes U+FFFD on each side. An `at` beyond the end gives
/// the whole text and an empty rest.
#[cfg_attr(not(test), expect(dead_code, reason = "the frame writer calls it"))]
pub(crate) fn utf16_split(text: &str, at: usize) -> (String, String) {
    let mut units = 0usize;
    for (offset, c) in text.char_indices() {
        let width = c.len_utf16();
        if units >= at {
            let (head, rest) = text.split_at_checked(offset).unwrap_or((text, ""));
            return (head.to_owned(), rest.to_owned());
        }
        if units.saturating_add(width) > at {
            // `at` falls between the two halves of a surrogate pair.
            let head = text.get(..offset).unwrap_or_default();
            let rest = text
                .get(offset.saturating_add(c.len_utf8())..)
                .unwrap_or_default();
            return (format!("{head}\u{FFFD}"), format!("\u{FFFD}{rest}"));
        }
        units = units.saturating_add(width);
    }
    (text.to_owned(), String::new())
}

/// Append `JSON.stringify(text)` to `out`: the text in double quotes, with `"` and `\` escaped,
/// U+0008, U+0009, U+000A, U+000C and U+000D as `\b`, `\t`, `\n`, `\f` and `\r`, and the other
/// code points below U+0020 as `\u00xx` in lowercase hex. Everything else is written as it is.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the canonical JSON writer calls it")
)]
pub(crate) fn json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0C}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}
