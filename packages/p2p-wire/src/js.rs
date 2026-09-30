//! The parts of JavaScript that decide the reference's bytes and values on the wire: the key that
//! replaces a plain object's prototype, UTF-16 lengths, order and cuts, and Node's lossy UTF-8
//! decoding. How `BigInt` and `Number` read text and how `JSON.stringify` escapes a string are
//! `heartwood_crypto::utils::js`'s, re-exported here under the names the crate uses. The own-key
//! order of a plain object is kept by [`Plugins`](crate::messages::peer::Plugins), its one user.

use std::cmp::Ordering;

pub(crate) use heartwood_crypto::utils::js::{
    BigIntError, json_string, parse_bigint, reads_as_zero,
};

#[cfg(test)]
mod tests;

/// The key whose assignment replaces a plain object's prototype instead of adding an own key.
pub(crate) const PROTO_KEY: &str = "__proto__";

/// The text of `bytes` as Node's `Buffer.toString()` decodes it: each maximal invalid UTF-8
/// subpart becomes U+FFFD.
pub(crate) fn utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The length of `text` in UTF-16 code units, JavaScript's `text.length`.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The order of JavaScript's default `sort()` on strings: by UTF-16 code units.
pub(crate) fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `text.slice(0, at)` and `text.slice(at)` in UTF-16 code units, as `Buffer.from` then writes
/// them: a surrogate pair cut in half becomes U+FFFD on each side. An `at` beyond the end gives
/// the whole text and an empty rest.
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
