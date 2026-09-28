//! PostgreSQL's `jsonb` containment and order, on the stored asset's JSON text.
//!
//! The reference keeps the asset in a `jsonb` column: its `asset` criteria is `asset @> value`,
//! and an `orderBy` on `asset` sorts by `jsonb`'s order. The store keeps the JSON text, and these
//! functions decide both as PostgreSQL does.

use serde_json::{Map, Number, Value};

use crate::filters::asset::Casts;

/// Whether the stored asset `asset` contains `criterion`, as PostgreSQL's `jsonb @>` decides it.
///
/// An object contains an object when it has each of its keys with a value that contains the
/// other's; an array contains an array when each element of the other is equal to (a scalar) or
/// contained in (an array or an object) one of its own, in any order; scalars contain an equal
/// scalar of the same type (numbers compare by value). A top-level array also contains a
/// top-level scalar that is one of its elements.
pub fn contains(asset: &Value, criterion: &Value) -> bool {
    contains_alternative(asset, criterion, &Casts::default())
}

/// Whether the stored asset `asset` contains one of the alternatives of the `asset` criteria
/// `criterion` ([`crate::filters::asset_alternatives`]), whose texts in `casts` also stand for
/// their casts, as [`contains`] decides it for each alternative, without building them.
///
/// Containment holds when each key of an object and each element of an array is contained, each
/// by its own part of the criteria, and the alternatives pick a value for each text on its own:
/// so one alternative is contained exactly when each part is contained with one of its own
/// choices, that is when each text of `casts` is matched by a scalar equal to it or to its cast.
pub fn contains_alternative(asset: &Value, criterion: &Value, casts: &Casts) -> bool {
    match (asset, criterion) {
        (Value::Array(elements), scalar) if is_scalar(scalar) => elements
            .iter()
            .any(|element| leaf_eq(element, scalar, casts)),
        _ => deep_contains(asset, criterion, casts),
    }
}

/// `jsonb`'s containment below the top level, where types must match.
fn deep_contains(value: &Value, contained: &Value, casts: &Casts) -> bool {
    match (value, contained) {
        (Value::Object(value), Value::Object(contained)) => {
            value.len() >= contained.len()
                && contained.iter().all(|(key, inner)| {
                    value.get(key).is_some_and(|own| match (own, inner) {
                        (own, inner) if is_scalar(own) && is_scalar(inner) => {
                            leaf_eq(own, inner, casts)
                        }
                        (own, inner) if !is_scalar(own) && !is_scalar(inner) => {
                            deep_contains(own, inner, casts)
                        }
                        _ => false,
                    })
                })
        }
        (Value::Array(value), Value::Array(contained)) => contained.iter().all(|inner| {
            if is_scalar(inner) {
                value.iter().any(|own| leaf_eq(own, inner, casts))
            } else {
                value
                    .iter()
                    .any(|own| !is_scalar(own) && deep_contains(own, inner, casts))
            }
        }),
        (value, contained) if is_scalar(value) && is_scalar(contained) => {
            leaf_eq(value, contained, casts)
        }
        _ => false,
    }
}

/// Whether the scalar `own` matches the criteria's scalar `inner`: equal to it, or, for a text of
/// `casts`, equal to its cast.
fn leaf_eq(own: &Value, inner: &Value, casts: &Casts) -> bool {
    scalar_eq(own, inner)
        || matches!(inner, Value::String(text) if casts.get(text).is_some_and(|cast| scalar_eq(own, cast)))
}

/// Whether `value` is neither an array nor an object.
fn is_scalar(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

/// Whether two scalars are equal as `jsonb` compares them: the same type, and numbers by value.
fn scalar_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => number_eq(a, b),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

/// Whether two JSON numbers have the same value.
fn number_eq(a: &Number, b: &Number) -> bool {
    match (a.as_i64(), b.as_i64(), a.as_u64(), b.as_u64()) {
        (Some(a), Some(b), _, _) => a == b,
        (_, _, Some(a), Some(b)) => a == b,
        _ => a.as_f64() == b.as_f64(),
    }
}

/// The ranks of `jsonb`'s value types in its order: null, string, number, boolean, then arrays,
/// then objects.
const NULL: u8 = 0x00;
const STRING: u8 = 0x01;
const NUMBER: u8 = 0x02;
const BOOLEAN: u8 = 0x03;
const ARRAY: u8 = 0x10;
const OBJECT: u8 = 0x11;

/// A key whose byte order is `jsonb`'s order of `value`: `ORDER BY asset` sorts by it.
///
/// `jsonb` compares two values token by token: containers first by type, then by their number of
/// elements or pairs, then element by element, or key and value by key and value with the keys in
/// `jsonb`'s storage order (shorter keys first, then by bytes); scalars by type rank, then by value
/// (strings by bytes, numbers by value, `false` before `true`). A top-level scalar is a
/// one-element array to `jsonb`, ordered before a real array of one element.
pub fn jsonb_order_key(value: &Value) -> Vec<u8> {
    let mut key = Vec::new();
    if is_scalar(value) {
        key.push(ARRAY);
        key.extend_from_slice(&1_u32.to_be_bytes());
        key.push(0);
        push_scalar(&mut key, value);
    } else {
        push_value(&mut key, value);
    }
    key
}

/// Append the tokens of `value` below the top level.
fn push_value(key: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Array(elements) => {
            key.push(ARRAY);
            key.extend_from_slice(&length(elements.len()).to_be_bytes());
            key.push(1);
            for element in elements {
                push_value(key, element);
            }
        }
        Value::Object(map) => {
            key.push(OBJECT);
            key.extend_from_slice(&length(map.len()).to_be_bytes());
            for (name, inner) in storage_order(map) {
                push_string(key, name);
                push_value(key, inner);
            }
        }
        scalar => push_scalar(key, scalar),
    }
}

/// Append one scalar token.
fn push_scalar(key: &mut Vec<u8>, value: &Value) {
    match value {
        Value::String(text) => push_string(key, text),
        Value::Number(number) => {
            key.push(NUMBER);
            key.extend_from_slice(&ordered_bits(number.as_f64().unwrap_or(0.0)).to_be_bytes());
        }
        Value::Bool(flag) => {
            key.push(BOOLEAN);
            key.push(u8::from(*flag));
        }
        Value::Null | Value::Array(_) | Value::Object(_) => key.push(NULL),
    }
}

/// Append a string token: its bytes with every zero byte escaped, then a terminator that sorts
/// before any byte, so a shorter string sorts before a longer one that starts with it.
fn push_string(key: &mut Vec<u8>, text: &str) {
    key.push(STRING);
    for &byte in text.as_bytes() {
        key.push(byte);
        if byte == 0 {
            key.push(0xff);
        }
    }
    key.extend_from_slice(&[0, 0]);
}

/// A container's size as a token field; beyond `u32` it saturates (no stored asset comes near).
fn length(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// The bits of `value`, changed so that their unsigned order is the numeric order.
fn ordered_bits(value: f64) -> u64 {
    // -0 and +0 are equal numbers.
    let bits = (value + 0.0).to_bits();
    if bits >> 63 == 1 {
        !bits
    } else {
        bits | (1 << 63)
    }
}

/// The entries of `map` in `jsonb`'s storage order: shorter keys first, then by bytes.
fn storage_order(map: &Map<String, Value>) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    entries
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn objects_contain_subsets_of_their_pairs() {
        let asset = json!({"votes": {"alice": 60.5, "bob": 39.5}});
        assert!(contains(&asset, &json!({})));
        assert!(contains(&asset, &json!({"votes": {}})));
        assert!(contains(&asset, &json!({"votes": {"alice": 60.5}})));
        assert!(contains(&asset, &json!({"votes": {"alice": 60.50}})));
        assert!(!contains(&asset, &json!({"votes": {"alice": "60.5"}})));
        assert!(!contains(&asset, &json!({"votes": {"carol": 1}})));
        assert!(!contains(&asset, &json!({"votes": []})));
        assert!(!contains(&asset, &json!({"other": {}})));
    }

    #[test]
    fn arrays_contain_their_elements_in_any_order() {
        let asset = json!({"transfers": [
            {"amount": "10", "recipientId": "A"},
            {"amount": "20", "recipientId": "B"},
        ]});
        assert!(contains(&asset, &json!({"transfers": []})));
        assert!(contains(
            &asset,
            &json!({"transfers": [{"recipientId": "B"}]})
        ));
        assert!(contains(
            &asset,
            &json!({"transfers": [{"recipientId": "B"}, {"amount": "10"}]})
        ));
        assert!(!contains(
            &asset,
            &json!({"transfers": [{"recipientId": "B", "amount": "10"}]})
        ));
        // Below the top level, an array never contains a bare scalar, nor an object an array.
        assert!(!contains(
            &asset,
            &json!({"transfers": {"recipientId": "B"}})
        ));
        assert!(!contains(&asset, &json!({"transfers": "B"})));
        assert!(contains(&json!([1, "a", [2]]), &json!([[2], 1])));
        assert!(!contains(&json!([1, "a"]), &json!([[1]])));
    }

    #[test]
    fn a_top_level_array_contains_a_scalar_element() {
        assert!(contains(&json!(["a", "b"]), &json!("b")));
        assert!(!contains(&json!(["a", "b"]), &json!("c")));
        assert!(contains(&json!("b"), &json!("b")));
        assert!(!contains(&json!("b"), &json!(["b"])));
        assert!(!contains(&json!({"a": 1}), &json!("a")));
        assert!(contains(&json!(null), &json!(null)));
    }

    fn sorted(values: &[Value]) -> Vec<Value> {
        let mut values = values.to_vec();
        values.sort_by_key(jsonb_order_key);
        values
    }

    #[test]
    fn the_order_key_sorts_as_jsonb_does() {
        // Types: null < string < number < boolean < array < object.
        assert_eq!(
            sorted(&[
                json!({}),
                json!([]),
                json!(true),
                json!(1),
                json!("a"),
                json!(null)
            ]),
            [
                json!([]),
                json!(null),
                json!("a"),
                json!(1),
                json!(true),
                json!({})
            ]
        );
        // Objects by size, then by key and value in storage order.
        assert_eq!(
            sorted(&[
                json!({"a": 1, "b": 1}),
                json!({"bb": 0}),
                json!({"a": 2}),
                json!({"a": 1})
            ]),
            [
                json!({"a": 1}),
                json!({"a": 2}),
                json!({"bb": 0}),
                json!({"a": 1, "b": 1})
            ]
        );
        // Strings by bytes, a prefix first; numbers by value.
        assert_eq!(
            sorted(&[json!(["ab"]), json!(["a"]), json!(["B"]), json!(["a\u{0}"])]),
            [json!(["B"]), json!(["a"]), json!(["a\u{0}"]), json!(["ab"])]
        );
        assert_eq!(
            sorted(&[json!([10]), json!([9.5]), json!([-1]), json!([0])]),
            [json!([-1]), json!([0]), json!([9.5]), json!([10])]
        );
        // A scalar value before a container value at the same place.
        assert_eq!(
            sorted(&[json!({"a": [1]}), json!({"a": 5}), json!({"a": {"b": 1}})]),
            [json!({"a": 5}), json!({"a": [1]}), json!({"a": {"b": 1}})]
        );
    }

    /// Containments with the answers of PostgreSQL 14 (`jsonb @> jsonb`), from a run of several
    /// thousand random cases that all agreed.
    #[test]
    fn contains_as_postgresql_does() {
        let cases: &[(&str, &str, bool)] = &[
            ("{\"x\":0,\"amount\":[]}", "{}", true),
            ("[0,{},[]]", "[[]]", true),
            (
                "[[],1.5,{\"recipientId\":\"\",\"x\":[10,10],\"a\":10}]",
                "[1.5]",
                true,
            ),
            (
                "{\"a\":{\"bb\":[],\"a\":\"A\"},\"bb\":1.5,\"c\":true}",
                "{\"a\":{\"bb\":[],\"a\":\"A\"},\"bb\":1.5,\"c\":true}",
                true,
            ),
            ("[\"\"]", "\"\"", true),
            ("[\"A\",{}]", "[]", true),
            ("[{\"b\":{}},10,null]", "null", true),
            (
                "{\"b\":[0,\"ab\"],\"amount\":{\"amount\":{},\"b\":1,\"a\":false},\"a\":{\"amount\":true}}",
                "{\"b\":[0]}",
                true,
            ),
            (
                "[{\"c\":\"a\"},[{\"recipientId\":\"\",\"c\":10}]]",
                "[{},{}]",
                true,
            ),
            ("[1.5,null]", "[]", true),
            (
                "{\"a\":{\"amount\":{\"a\":\"A\",\"c\":\"a\",\"b\":\"A\"},\"c\":{}}}",
                "{\"a\":{\"amount\":{}}}",
                true,
            ),
            (
                "[{\"recipientId\":[\"ab\",1.5],\"amount\":[]},[{\"c\":-1,\"amount\":\"b\",\"bb\":\"ab\"},-1,\"ab\"],\"\"]",
                "[{}]",
                true,
            ),
            ("[1.5,null]", "[null]", true),
            ("null", "[{\"a\":null,\"x\":0}]", false),
            (
                "{\"c\":[[-1,\"b\"],true,true]}",
                "[{\"b\":{},\"amount\":10,\"bb\":null}]",
                false,
            ),
            ("[1.5,true]", "[\"\",\"ab\"]", false),
            (
                "[{},-1,[true,{},[-1,\"a\",1.5]]]",
                "{\"c\":{},\"b\":[{}],\"amount\":\"ab\"}",
                false,
            ),
            (
                "{\"b\":{\"a\":\"ab\",\"recipientId\":[-1],\"b\":[\"ab\"]}}",
                "{\"a\":{\"c\":\"50\",\"x\":10,\"b\":\"50\"}}",
                false,
            ),
            (
                "[{\"recipientId\":1.5,\"amount\":[1,10]},false,\"\"]",
                "[[\"b\",-1],{}]",
                false,
            ),
            (
                "[{\"recipientId\":{},\"bb\":null,\"a\":\"a\"},0]",
                "[{\"b\":{}},10,null]",
                false,
            ),
            ("[{},0,[[false],-1,false]]", "{\"amount\":10}", false),
            (
                "[[{\"a\":10}],1.5]",
                "{\"x\":10,\"bb\":true,\"c\":{\"x\":{},\"recipientId\":[\"b\"],\"b\":-1}}",
                false,
            ),
            ("\"\"", "[{\"b\":1,\"amount\":{}}]", false),
            (
                "[{\"amount\":{\"b\":1,\"a\":\"50\"}}]",
                "[{\"amount\": {}}]",
                true,
            ),
            ("[\"50\",10,[[\"a\"]]]", "[\"50\", [[\"a\"], []]]", true),
            (
                "[[{\"amount\":true},[null,\"b\",\"a\"]],\"ab\"]",
                "[[{\"amount\": true}]]",
                true,
            ),
            (
                "[{\"x\":\"\"},\"b\",{\"bb\":{},\"a\":[\"A\",null]}]",
                "[\"b\", {\"bb\": {}, \"a\": [\"A\", null, null]}]",
                true,
            ),
            ("{\"b\":false,\"c\":1.5}", "{\"c\": 1.5}", true),
            (
                "{\"bb\":\"ab\",\"c\":{\"b\":[\"a\",10,\"a\"],\"amount\":10}}",
                "{\"bb\": \"ab\", \"c\": {\"amount\": 10}}",
                true,
            ),
            (
                "{\"b\":\"ab\",\"bb\":10,\"amount\":1}",
                "{\"b\": \"ab\", \"amount\": 1}",
                true,
            ),
            ("[1,{\"c\":false}]", "[{}, {}]", true),
            ("[\"a\",[\"ab\"],\"A\"]", "[\"A\", []]", true),
            (
                "[[1.5],[[true,\"ab\",\"b\"]],[]]",
                "[[1.5], [], [[true, \"b\"]]]",
                true,
            ),
            (
                "[[{\"recipientId\":1.5,\"b\":10},[],{\"amount\":1}]]",
                "[]",
                true,
            ),
            (
                "[{\"a\":[],\"c\":[false,\"ab\"],\"x\":{\"b\":\"\"}}]",
                "[{\"a\": []}]",
                true,
            ),
            ("[\"\",-1]", "[-1.0]", true),
            (
                "{\"b\":{\"b\":[],\"a\":-1,\"c\":[\"50\",\"b\"]},\"a\":\"A\",\"bb\":\"b\"}",
                "{\"b\": {\"a\": -1.0}, \"bb\": \"b\"}",
                true,
            ),
            ("[{\"b\":{}},10,null]", "[10.0, 10.0]", true),
            (
                "[{\"amount\":10,\"c\":\"50\",\"b\":\"A\"},true]",
                "[true, {\"amount\": 10}]",
                true,
            ),
            ("[-1]", "[-1, -1.0]", true),
            ("[-1]", "[]", true),
            ("[1,\"b\"]", "[\"b\"]", true),
            ("[[{\"c\":10}]]", "[[{\"c\": 10}], [{}, {}]]", true),
            (
                "{\"b\":1.5,\"bb\":\"b\",\"a\":{\"a\":-1}}",
                "{\"bb\": \"b\", \"a\": {}}",
                true,
            ),
            (
                "{\"bb\":1,\"a\":true,\"b\":1}",
                "{\"bb\": 1, \"a\": \"zz\", \"b\": 1.0}",
                false,
            ),
            ("[\"\",true,-1]", "[\"\", \"zz\"]", false),
            (
                "[{\"x\":false},[\"\",[\"50\",null],\"\"]]",
                "[[\"zz\", [\"50\", null]], {\"x\": false}]",
                false,
            ),
            (
                "[{\"x\":false},[\"\",[\"50\",null],\"\"]]",
                "[{\"x\": \"zz\"}, [\"\", [\"50\", null]]]",
                false,
            ),
            ("{\"bb\":\"50\"}", "{\"bb\": \"zz\"}", false),
            (
                "[[\"ab\",[true,\"50\",\"a\"],{}],{}]",
                "[[\"zz\"], {}]",
                false,
            ),
            (
                "{\"b\":[{\"recipientId\":null,\"bb\":1},null],\"x\":\"50\"}",
                "{\"b\": [null, {\"bb\": 1.0}], \"x\": \"zz\"}",
                false,
            ),
            (
                "{\"x\":{\"recipientId\":[\"ab\"]}}",
                "{\"x\": {\"recipientId\": [\"zz\", \"ab\"]}}",
                false,
            ),
            ("[[\"b\",-1],{}]", "[[\"zz\", -1]]", false),
            (
                "[{},0,[[false],-1,false]]",
                "[0, {}, [\"zz\", [\"zz\"], false]]",
                false,
            ),
            (
                "[[{\"b\":1,\"a\":\"\"},{\"a\":0,\"bb\":1.5,\"amount\":null}],10,10]",
                "[[{\"b\": 1.0, \"a\": \"zz\"}], 10]",
                false,
            ),
            ("{\"bb\":\"a\"}", "{\"bb\": \"zz\"}", false),
            ("[[],\"\",{\"c\":{}}]", "[{\"c\": {}}, \"zz\"]", false),
            (
                "[true,{\"a\":[null,0],\"x\":1},{\"bb\":[true],\"c\":null}]",
                "[\"zz\"]",
                false,
            ),
            (
                "{\"bb\":\"\",\"x\":null,\"c\":\"A\"}",
                "{\"bb\": \"\", \"x\": \"zz\", \"c\": \"A\"}",
                false,
            ),
            (
                "[[{\"recipientId\":1.5,\"b\":10},[],{\"amount\":1}]]",
                "[[[], {\"amount\": \"zz\"}]]",
                false,
            ),
            ("[[false,{}]]", "[[\"zz\"]]", false),
            (
                "{\"bb\":\"ab\",\"c\":\"50\"}",
                "{\"bb\": \"zz\", \"c\": \"50\"}",
                false,
            ),
            ("[[{\"x\":\"ab\"},true],1.5]", "[[\"zz\"]]", false),
        ];
        for (asset, criterion, expected) in cases {
            let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap();
            assert_eq!(
                contains(&parse(asset), &parse(criterion)),
                *expected,
                "{asset} @> {criterion}"
            );
        }
    }

    /// Values in the order of PostgreSQL 14's `ORDER BY value::jsonb` (C locale), a sample of a
    /// sorted run of several hundred values that agreed.
    #[test]
    fn sorts_as_postgresql_does() {
        let ordered: Vec<Value> = [
            "-1",
            "[\"a\"]",
            "[0]",
            "[1]",
            "[[null]]",
            "[[false,{}]]",
            "[[[\"a\"],{},[\"A\",1.5]]]",
            "[{}]",
            "[{\"a\":null,\"x\":0}]",
            "[{\"a\":[],\"c\":[false,\"ab\"],\"x\":{\"b\":\"\"}}]",
            "[{\"b\":{},\"amount\":10,\"bb\":null}]",
            "[\"\",-1]",
            "[\"50\",\"b\"]",
            "[[{\"amount\":true},[null,\"b\",\"a\"]],\"ab\"]",
            "[[{\"x\":\"ab\"},true],1.5]",
            "[{\"x\":false},[\"\",[\"50\",null],\"\"]]",
            "[{\"c\":{},\"recipientId\":{\"bb\":0}},{\"c\":[],\"recipientId\":[0,\"50\",\"A\"]}]",
            "[\"A\",[{\"b\":true},{\"amount\":\"A\",\"bb\":\"ab\",\"a\":\"ab\"},{}],{\"recipientId\":\"A\"}]",
            "[-1,[\"a\",{\"c\":false,\"recipientId\":false,\"a\":1.5},{\"amount\":\"50\",\"recipientId\":\"ab\"}],{\"bb\":{\"recipientId\":\"b\"}}]",
            "[1,1.5,\"a\"]",
            "[10,[1],[10,[null]]]",
            "[false,null,[10,{\"recipientId\":1.5,\"a\":\"A\"}]]",
            "[[1.5],[[true,\"ab\",\"b\"]],[]]",
            "[{},\"a\",\"b\"]",
            "[{},-1,[true,{},[-1,\"a\",1.5]]]",
            "[{\"b\":{}},10,null]",
            "{\"b\":null}",
            "{\"bb\":{}}",
            "{\"bb\":{\"a\":1,\"c\":\"a\"}}",
            "{\"c\":{\"c\":{},\"b\":{\"bb\":1.5}}}",
            "{\"recipientId\":[0,[true]]}",
            "{\"x\":{\"recipientId\":false,\"a\":[-1],\"b\":{\"b\":true}}}",
            "{\"a\":{},\"bb\":[\"A\"]}",
            "{\"a\":{\"b\":-1,\"amount\":{}},\"amount\":\"50\"}",
            "{\"b\":\"50\",\"c\":{\"c\":{\"recipientId\":1,\"b\":1},\"b\":{\"b\":10,\"x\":1.5},\"x\":true}}",
            "{\"amount\":1.5,\"bb\":{\"x\":true,\"a\":\"a\"}}",
            "{\"amount\":\"\",\"c\":[]}",
            "{\"recipientId\":10,\"c\":{\"a\":false}}",
            "{\"recipientId\":null,\"amount\":\"a\",\"a\":\"50\"}",
            "{\"c\":false,\"a\":-1,\"recipientId\":{\"amount\":{\"b\":\"b\",\"a\":\"ab\"},\"b\":[]}}",
            "{\"recipientId\":10,\"bb\":{\"bb\":1.5},\"a\":[{\"b\":\"50\",\"recipientId\":0,\"a\":10},\"50\",\"\"]}",
            "{\"x\":10,\"a\":{\"amount\":false},\"c\":\"a\"}",
            "{\"c\":{\"recipientId\":\"\",\"b\":{\"a\":\"A\",\"b\":0}},\"a\":{\"amount\":\"b\",\"x\":1.5,\"b\":{\"x\":\"a\"}},\"x\":[]}",
            "{\"a\":{\"recipientId\":\"\",\"bb\":[1,\"a\",\"ab\"],\"amount\":{\"recipientId\":\"50\",\"c\":-1,\"a\":\"b\"}},\"bb\":{\"a\":true,\"bb\":[-1,-1],\"c\":0},\"c\":{}}",
            "{\"bb\":1,\"c\":0,\"x\":\"b\"}",
        ]
        .iter()
        .map(|text| serde_json::from_str(text).unwrap())
        .collect();
        let mut shuffled = ordered.clone();
        shuffled.reverse();
        shuffled.rotate_left(7);
        assert_eq!(sorted(&shuffled), ordered);
    }
}
