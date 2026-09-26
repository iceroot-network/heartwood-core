//! The option merge: objects merge key by key, and arrays and every other value replace.

use serde_json::Value;

/// `source` merged onto `target`, as the reference merges component options: two objects merge
/// key by key (the target's keys first, then the source's new keys); anything else, arrays
/// included, is replaced by the source.
pub fn deep_merge(target: &Value, source: &Value) -> Value {
    let _ = (target, source);
    todo!()
}
