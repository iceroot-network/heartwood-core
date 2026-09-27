//! The reference's canonical JSON for the relay status that peers sign.
//!
//! The reference writes `JSON.stringify(value, keys)`, where `keys` is every key that its first
//! pass meets anywhere in `value`, sorted by UTF-16 code unit. Every key an object has is met, so
//! the result is `value` with the keys of every object sorted by UTF-16 code unit, whatever the
//! order of the `serde_json` map, with one quirk kept: the first pass records a key only when it
//! is not already `in` a plain object, so the names of `Object.prototype`'s own properties
//! (`constructor`, `toString`, `__proto__` and the others) never enter the list, and an object's
//! keys with those names are left out. Numbers and strings are written as `JSON.stringify` writes
//! them, with no whitespace.

use heartwood_crypto::utils::js;
use serde_json::Value;

use crate::js_object::PROTOTYPE_NAMES;

/// The canonical JSON text of `value`.
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value);
    out
}

fn write(out: &mut String, value: &Value) {
    match value {
        Value::Array(items) => {
            out.push('[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                write(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map
                .keys()
                .filter(|key| !PROTOTYPE_NAMES.contains(&key.as_str()))
                .collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (position, key) in keys.into_iter().enumerate() {
                if position > 0 {
                    out.push(',');
                }
                out.push_str(&js::stringify(&Value::String(key.clone()), 0));
                out.push(':');
                if let Some(item) = map.get(key) {
                    write(out, item);
                }
            }
            out.push('}');
        }
        scalar => out.push_str(&js::stringify(scalar, 0)),
    }
}
