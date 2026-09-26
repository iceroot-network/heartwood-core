//! The reference's canonical JSON for the relay status that peers sign.
//!
//! `JSON.stringify(value, keys)` where `keys` is every key that occurs anywhere in `value`,
//! sorted by UTF-16 code unit: every object's keys come out in that sorted order, whatever the
//! order of the `serde_json` map. Numbers are written as JavaScript writes them.

use serde_json::Value;

/// The canonical JSON text of `value`.
pub fn stringify(value: &Value) -> String {
    let _ = value;
    todo!()
}
