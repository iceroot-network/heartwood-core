//! The option merge of the reference's plugin configuration: `deepmerge` with arrays replaced.
//!
//! Two objects merge key by key: the target's keys first, then the source's new keys, in the order
//! of a JavaScript object. A key both have merges when the source's value is an object or an
//! array; any other source value replaces the target's. Arrays, and anything whose type differs
//! from the target's, replace the target. The library's quirks with the names of
//! `Object.prototype`'s own properties are kept: a source key such as `constructor` or `toString`
//! that the target object does not have is skipped, objects copied from either side lose such
//! keys (unless the target is not an object), and `__proto__` is never kept. The reference calls
//! the merge with two objects; a source that is not an object or an array replaces the target.

use heartwood_crypto::utils::js;
use serde_json::{Map, Value};

use crate::js_object::PROTOTYPE_NAMES;

/// `source` merged onto `target`, as the reference merges component options.
pub fn deep_merge(target: &Value, source: &Value) -> Value {
    match (target, source) {
        (_, Value::Array(_)) => source.clone(),
        (Value::Object(target), Value::Object(source)) => {
            let mut merged = Map::new();
            for (key, value) in target {
                insert(&mut merged, key, copy(value));
            }
            for (key, value) in source {
                match target.get(key) {
                    None if PROTOTYPE_NAMES.contains(&key.as_str()) => {}
                    Some(existing) if value.is_object() => {
                        insert(&mut merged, key, deep_merge(existing, value));
                    }
                    _ => insert(&mut merged, key, copy(value)),
                }
            }
            Value::Object(in_property_order(merged))
        }
        // A target that is not an object has no keys to protect: every source key is copied.
        (
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_),
            Value::Object(source),
        ) => {
            let mut merged = Map::new();
            for (key, value) in source {
                insert(&mut merged, key, copy(value));
            }
            Value::Object(in_property_order(merged))
        }
        _ => copy(source),
    }
}

/// `value` as the library copies it: objects rebuilt without the prototype names, arrays and
/// other values as they are.
fn copy(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let copied = map
                .iter()
                .filter(|(key, _)| !PROTOTYPE_NAMES.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), copy(value)))
                .collect();
            Value::Object(in_property_order(copied))
        }
        other => other.clone(),
    }
}

/// Set `key`, which JavaScript's assignment to `__proto__` never makes an own property.
fn insert(map: &mut Map<String, Value>, key: &str, value: Value) {
    if key != "__proto__" {
        map.insert(key.to_owned(), value);
    }
}

fn in_property_order(map: Map<String, Value>) -> Map<String, Value> {
    js::property_order(&map)
        .into_iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
