//! JavaScript number semantics, wherever the reference's numbers are observable.
//!
//! The reference reads JSON with `JSON.parse`, so every JSON number is an IEEE 754 double, and it
//! writes JSON with `JSON.stringify`, which prints numbers with `Number.prototype.toString`. This
//! module is the only place in the crate that models either.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use serde_json::{Number, Value};

/// The largest integer a double holds exactly, `2^53 − 1` (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// The double that `JSON.parse` gives for the JSON number `number`.
///
/// For example `9007199254740993` gives `2^53`, and `8.0000000000000001` gives `8`.
pub fn number_value(number: &Number) -> f64 {
    todo!()
}

/// Whether `value` is an integer from `-(2^53 − 1)` to `2^53 − 1` (`Number.isSafeInteger`).
pub fn is_safe_integer(value: f64) -> bool {
    todo!()
}

/// `value` as `Number.prototype.toString` prints it.
pub fn number_to_string(value: f64) -> String {
    todo!()
}

/// `value` as `JSON.stringify(value, null, indent)` writes it: `indent` spaces per level, keys in
/// the order of the map, numbers as [`number_to_string`] prints them.
pub fn stringify(value: &Value, indent: usize) -> String {
    todo!()
}
