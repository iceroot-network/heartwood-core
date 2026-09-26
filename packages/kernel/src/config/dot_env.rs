//! The reference's `.env` format.
//!
//! Parsing reads each line with `^([^=:#]+?)[=:](.*)`: the key is the trimmed text before the
//! first `=` or `:`, the value the rest with one pair of surrounding double quotes removed, then
//! trimmed. A line whose key or value is empty is skipped. A value of ASCII digits only is a
//! number, `true` and `false` are booleans, anything else is text. The keys are kept in the order
//! of a JavaScript object: array-index keys first, in numeric order, then the others in the order
//! they first appear; a later line with the same key replaces the value.

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
    let _ = text;
    todo!()
}

/// The `.env` text of `pairs`, one `key=value` line each, text values in double quotes, joined
/// with `\n` and no final newline.
pub fn stringify(pairs: &IndexMap<String, EnvValue>) -> String {
    let _ = pairs;
    todo!()
}
