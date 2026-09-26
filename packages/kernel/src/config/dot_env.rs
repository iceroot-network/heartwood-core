//! The reference's `.env` format.
//!
//! Parsing splits the text at `\n` and reads each line with `^([^=:#]+?)[=:](.*)`: the key is the
//! text before the first `=` or `:`, and a line whose key would hold a `#` is skipped. The value
//! is the rest of the line up to its first line break (`\r`, U+2028 or U+2029), with one pair of
//! surrounding double quotes removed when the whole value is quoted. Key and value are then
//! trimmed of JavaScript's white space, and a line whose key or value is empty is skipped. A
//! value of ASCII digits only is a number, `true` and `false` are booleans, anything else is text.
//! The keys are kept in the order of a JavaScript object: array-index keys first, in numeric
//! order, then the others in the order they first appear; a later line with the same key replaces
//! the value, and the key `__proto__` is never kept.

use heartwood_crypto::utils::js;
use indexmap::IndexMap;

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvValue {
    /// A value of ASCII digits only, read as JavaScript's `Number` reads it.
    Number(f64),
    /// `true` or `false`.
    Bool(bool),
    /// Any other value.
    Text(String),
}

/// The pairs of the `.env` text `text`.
pub fn parse(text: &str) -> IndexMap<String, EnvValue> {
    let mut pairs = IndexMap::new();
    for line in text.split('\n') {
        let Some(at) = line.find(['=', ':', '#']) else {
            continue;
        };
        let (Some(key), Some(rest)) = (line.get(..at), line.get(at.saturating_add(1)..)) else {
            continue;
        };
        if key.is_empty() || line.get(at..).is_some_and(|rest| rest.starts_with('#')) {
            continue;
        }
        let value = rest
            .split(['\r', '\u{2028}', '\u{2029}'])
            .next()
            .unwrap_or_default();
        let value = value
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(value);
        let (key, value) = (js_trim(key), js_trim(value));
        if key.is_empty() || value.is_empty() || key == "__proto__" {
            continue;
        }
        let parsed = if value.bytes().all(|b| b.is_ascii_digit()) {
            EnvValue::Number(value.parse().unwrap_or(f64::INFINITY))
        } else if let Ok(flag) = value.parse::<bool>() {
            EnvValue::Bool(flag)
        } else {
            EnvValue::Text(value.to_owned())
        };
        pairs.insert(key.to_owned(), parsed);
    }
    in_property_order(pairs)
}

/// The `.env` text of `pairs`, one `key=value` line each in the order of a JavaScript object's
/// keys, text values in double quotes, numbers as JavaScript prints them, joined with `\n` and no
/// final newline. A pair with an empty key is left out.
pub fn stringify(pairs: &IndexMap<String, EnvValue>) -> String {
    in_property_order(pairs.clone())
        .iter()
        .filter(|(key, _)| !key.is_empty())
        .map(|(key, value)| match value {
            EnvValue::Number(number) => format!("{key}={}", js::number_to_string(*number)),
            EnvValue::Bool(flag) => format!("{key}={flag}"),
            EnvValue::Text(text) => format!("{key}=\"{text}\""),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `pairs` with the array-index keys first, in numeric order, as a JavaScript object orders them.
fn in_property_order(pairs: IndexMap<String, EnvValue>) -> IndexMap<String, EnvValue> {
    let mut entries: Vec<(String, EnvValue)> = pairs.into_iter().collect();
    // A stable sort: the other keys keep their order after the indexes.
    entries.sort_by_key(|(key, _)| {
        js::is_array_index(key)
            .then(|| key.parse::<u64>().ok())
            .flatten()
            .unwrap_or(u64::MAX)
    });
    entries.into_iter().collect()
}

/// `text` without the white space and line breaks JavaScript's `String.prototype.trim` removes.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| {
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
    })
}
