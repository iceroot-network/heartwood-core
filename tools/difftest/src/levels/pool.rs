//! The judge of a pool session (`pool.run`): the oracle's answer to every step against Heartwood's.
//!
//! Each step gives a result and the pool's contents after it. A processor result's lists
//! (`accept`, `broadcast`, `invalid`, `excess`) are compared in order, its `errors` as an ordered
//! map of entry id to code and message, and the contents exactly. Messages are compared exactly,
//! with the rules of the documented differences:
//!
//! - the reason after "Invalid transaction data: " of an entry that does not decode is Heartwood's
//!   decoder's ([`Documented::DecodeReason`]): the code, the entry id, the place in the lists and
//!   the verdict must still be equal;
//! - a full pool that evicted a transaction and then refused the new one, as the oracle notes it
//!   ([`Documented::FullPoolEviction`]): Heartwood refuses the same entries without evicting, so
//!   its pool must be the reference's plus the evicted transactions; the later steps are not
//!   compared, since the two pools differ from there on purpose;
//! - entries of a removed type and JSON votes given as an array or a string
//!   ([`Documented::RemovedType`], [`Documented::VotesWithIndices`]), checked on the entries
//!   themselves: the step's lists and errors must be equal once those entries are left out on both
//!   sides, whatever ids they were given;
//! - a count in a message that the oracle's default locale groups otherwise
//!   ([`Documented::Locale`], shown by the oracle's `locale` note).

use std::collections::BTreeSet;

use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::utils::hex;
use serde_json::{Map, Value};

use crate::compare;
use crate::report::Outcome;
use crate::subject::{Documented, NOTES};

/// The prefix of the message of an entry that does not decode.
pub const DECODE_PREFIX: &str = "Invalid transaction data: ";

/// The removed types of type group 1.
const REMOVED_TYPES: [u64; 7] = [0, 3, 4, 5, 8, 9, 10];

/// The lists of a processor result.
const LISTS: [&str; 4] = ["accept", "broadcast", "invalid", "excess"];

/// Why an entry of a step is left out of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    RemovedType,
    VotesWithIndices,
}

/// The documented forms among the entries of a `process` step, with the ids either side may give
/// each: its index, the SHA-256 of its bytes, and for JSON the id it gives.
fn documented_entries(step: &Value) -> Vec<(Form, BTreeSet<String>)> {
    let Some(entries) = step.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let mut ids = BTreeSet::from([i.to_string()]);
        let form = if let Some(text) = entry.get("hex").and_then(Value::as_str) {
            let bytes = hex::decode(text).unwrap_or_default();
            ids.insert(hex::encode(&sha256(&bytes)));
            is_removed_bytes(&bytes).then_some(Form::RemovedType)
        } else if let Some(json) = entry.get("json") {
            if let Some(id) = json.get("id").and_then(Value::as_str) {
                ids.insert(id.to_owned());
            }
            let removed = json.get("typeGroup").and_then(Value::as_u64) == Some(1)
                && json
                    .get("type")
                    .and_then(Value::as_u64)
                    .is_some_and(|t| REMOVED_TYPES.contains(&t));
            let votes = json.pointer("/asset/votes");
            let indexed = matches!(votes, Some(Value::Array(a)) if !a.is_empty())
                || matches!(votes, Some(Value::String(s)) if !s.is_empty());
            if removed {
                Some(Form::RemovedType)
            } else if indexed {
                Some(Form::VotesWithIndices)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(form) = form {
            out.push((form, ids));
        }
    }
    out
}

/// Whether `bytes` have the standard header of a removed type of type group 1.
fn is_removed_bytes(bytes: &[u8]) -> bool {
    let group = bytes
        .get(3..7)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes);
    let kind = bytes
        .get(7..9)
        .and_then(|b| <[u8; 2]>::try_from(b).ok())
        .map(u16::from_le_bytes);
    bytes.first() == Some(&0xff)
        && group == Some(1)
        && kind.is_some_and(|k| REMOVED_TYPES.contains(&u64::from(k)))
}

/// A processor result without the entries whose ids are in `ids`, and with the ids of those
/// entries' transactions (the second-loop ids of `also`) left out too.
fn without_entries(result: &Value, ids: &BTreeSet<String>) -> Value {
    let mut out = result.clone();
    let Some(object) = out.as_object_mut() else {
        return out;
    };
    for list in LISTS {
        if let Some(Value::Array(items)) = object.get_mut(list) {
            items.retain(|v| v.as_str().is_none_or(|id| !ids.contains(id)));
        }
    }
    if let Some(Value::Object(errors)) = object.get_mut("errors") {
        errors.retain(|id, _| !ids.contains(id));
    }
    if object
        .get("errors")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        object.shift_remove("errors");
    }
    out
}

/// How the messages of two processor results compare, when everything else is equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Messages {
    Equal,
    DecodeReason,
    Different,
}

/// Compare the errors' messages of two processor results whose other parts are equal: equal, or
/// different only in the reason of entries that did not decode.
fn messages(expected: &Value, actual: &Value) -> Messages {
    let (Some(a), Some(b)) = (
        expected.get("errors").and_then(Value::as_object),
        actual.get("errors").and_then(Value::as_object),
    ) else {
        return Messages::Equal;
    };
    let mut out = Messages::Equal;
    for (id, theirs) in a {
        let ours = &b[id];
        let (Some(t), Some(o)) = (
            theirs.get("message").and_then(Value::as_str),
            ours.get("message").and_then(Value::as_str),
        ) else {
            return Messages::Different;
        };
        if t == o {
            continue;
        }
        let both_bad_data = theirs.get("type") == Some(&Value::from("ERR_BAD_DATA"))
            && ours.get("type") == Some(&Value::from("ERR_BAD_DATA"));
        if both_bad_data && t.starts_with(DECODE_PREFIX) && o.starts_with(DECODE_PREFIX) {
            out = Messages::DecodeReason;
        } else {
            return Messages::Different;
        }
    }
    out
}

/// `result` with every error message replaced by its code, to compare the rest.
fn without_messages(result: &Value) -> Value {
    let mut out = result.clone();
    if let Some(Value::Object(errors)) = out.get_mut("errors") {
        for error in errors.values_mut() {
            if let Some(o) = error.as_object_mut() {
                o.shift_remove("message");
            }
        }
    }
    out
}

/// The ids of a contents' `all`, as a set.
fn all_ids(contents: &Value) -> BTreeSet<String> {
    contents
        .get("all")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether Heartwood's step shows the fixed eviction the oracle noted: the entries the reference
/// refused after an eviction are refused by Heartwood too, and Heartwood's pool is the
/// reference's plus the evicted transactions.
fn eviction_holds(notes: &Value, expected: &Value, actual: &Value) -> bool {
    let Some(evictions) = notes.get("evictions").and_then(Value::as_array) else {
        return false;
    };
    let mut want = all_ids(&expected["contents"]);
    for eviction in evictions {
        let refused = eviction.get("id").and_then(Value::as_str).unwrap_or("");
        let ours_refused = actual["result"]["invalid"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(refused)));
        if !ours_refused {
            return false;
        }
        for id in eviction
            .get("evicted")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            want.insert(id.as_str().unwrap_or("").to_owned());
        }
    }
    all_ids(&actual["contents"]) == want
}

/// Judge Heartwood's answer `actual` to the pool session `input` against the oracle's
/// `expected`.
pub fn judge(input: &Value, expected: &Value, actual: &Value) -> Outcome {
    let steps = input
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let theirs = expected
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ours = actual
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if theirs.len() != ours.len() || theirs.len() != steps.len() {
        return Outcome::HarnessError(format!(
            "{} steps, the oracle answered {} and Heartwood {}",
            steps.len(),
            theirs.len(),
            ours.len()
        ));
    }
    let locale = expected[NOTES]["locale"] == Value::Bool(true);
    let mut documented: Option<Documented> = None;
    for (i, ((step, a), b)) in steps.iter().zip(&theirs).zip(&ours).enumerate() {
        let mut a_result = a["result"].clone();
        let notes = a_result
            .as_object_mut()
            .and_then(|o| o.shift_remove(NOTES))
            .unwrap_or(Value::Null);
        if notes["fullPoolEviction"] == Value::Bool(true) {
            return if eviction_holds(&notes, a, b) {
                Outcome::Documented(Documented::FullPoolEviction.reason().to_owned())
            } else {
                Outcome::Mismatch(prefixed(
                    i,
                    compare::diff(
                        &Value::Array(vec![a.clone()]),
                        &Value::Array(vec![b.clone()]),
                    ),
                ))
            };
        }
        let contents = compare::diff(&a["contents"], &b["contents"]);
        let b_result = &b["result"];
        let same_shape = compare::diff(&without_messages(&a_result), &without_messages(b_result));
        let (same_shape, forms) = if same_shape.is_empty() {
            (true, Vec::new())
        } else {
            let entries = documented_entries(step);
            let ids: BTreeSet<String> = entries.iter().flat_map(|(_, ids)| ids.clone()).collect();
            let equal = !entries.is_empty()
                && compare::diff(
                    &without_messages(&without_entries(&a_result, &ids)),
                    &without_messages(&without_entries(b_result, &ids)),
                )
                .is_empty();
            if !equal {
                return Outcome::Mismatch(prefixed(i, same_shape));
            }
            (false, entries.into_iter().map(|(form, _)| form).collect())
        };
        if !contents.is_empty() {
            return Outcome::Mismatch(prefixed(i, contents));
        }
        if !same_shape {
            documented.get_or_insert(if forms.contains(&Form::RemovedType) {
                Documented::RemovedType
            } else {
                Documented::VotesWithIndices
            });
            continue;
        }
        match messages(&a_result, b_result) {
            Messages::Equal => {}
            Messages::DecodeReason => {
                documented.get_or_insert(Documented::DecodeReason);
            }
            Messages::Different if locale => {
                documented.get_or_insert(Documented::Locale);
            }
            Messages::Different => {
                return Outcome::Mismatch(prefixed(i, compare::diff(&a_result, b_result)));
            }
        }
    }
    match documented {
        Some(difference) => Outcome::Documented(difference.reason().to_owned()),
        None => Outcome::Pass,
    }
}

/// `differences` with the step's index in their paths.
fn prefixed(step: usize, differences: Vec<compare::Difference>) -> Vec<compare::Difference> {
    differences
        .into_iter()
        .map(|d| match d {
            compare::Difference::Value {
                path,
                expected,
                actual,
            } => compare::Difference::Value {
                path: format!("step {step} {path}"),
                expected,
                actual,
            },
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(results: Vec<Value>) -> Value {
        json!({ "results": results })
    }

    fn step(result: Value, all: &[&str]) -> Value {
        json!({"result": result, "contents": {"all": all, "size": all.len()}})
    }

    fn process(entries: Value) -> Value {
        json!({"steps": [{"op": "process", "entries": entries, "now": 0}]})
    }

    #[test]
    fn lists_maps_and_messages_are_compared_exactly() {
        let input = process(json!([{"hex": "00"}]));
        let result = json!({"accept": ["a"], "broadcast": ["a"], "invalid": ["b"], "excess": [],
                            "errors": {"b": {"type": "ERR_LOW_FEE", "message": "b fee is too low"}}});
        let expected = run(vec![step(result.clone(), &["a"])]);
        assert_eq!(judge(&input, &expected, &expected), Outcome::Pass);
        let mut order = result.clone();
        order["accept"] = json!(["a", "c"]);
        assert!(judge(&input, &expected, &run(vec![step(order, &["a"])])).is_failure());
        let mut message = result.clone();
        message["errors"]["b"]["message"] = json!("b fee is low");
        assert!(judge(&input, &expected, &run(vec![step(message, &["a"])])).is_failure());
        let mut code = result.clone();
        code["errors"]["b"]["type"] = json!("ERR_APPLY");
        assert!(judge(&input, &expected, &run(vec![step(code, &["a"])])).is_failure());
        assert!(judge(&input, &expected, &run(vec![step(result.clone(), &[])])).is_failure());
        // The order of the errors map counts.
        let two = json!({"accept": [], "broadcast": [], "invalid": ["x", "y"], "excess": [],
                         "errors": {"x": {"type": "ERR_OTHER", "message": "m"}, "y": {"type": "ERR_OTHER", "message": "m"}}});
        let swapped = json!({"accept": [], "broadcast": [], "invalid": ["x", "y"], "excess": [],
                             "errors": {"y": {"type": "ERR_OTHER", "message": "m"}, "x": {"type": "ERR_OTHER", "message": "m"}}});
        assert!(
            judge(
                &input,
                &run(vec![step(two, &[])]),
                &run(vec![step(swapped, &[])])
            )
            .is_failure()
        );
    }

    #[test]
    fn a_decode_reason_is_documented_and_the_rest_compared() {
        let input = process(json!([{"hex": "ff"}]));
        let result = |message: &str| {
            json!({"accept": [], "broadcast": [], "invalid": ["0"], "excess": [],
                   "errors": {"0": {"type": "ERR_BAD_DATA", "message": message}}})
        };
        let expected = run(vec![step(
            result("Invalid transaction data: Read over buffer boundary"),
            &[],
        )]);
        let ours = run(vec![step(
            result("Invalid transaction data: the bytes end early"),
            &[],
        )]);
        assert_eq!(
            judge(&input, &expected, &ours),
            Outcome::Documented(Documented::DecodeReason.reason().into())
        );
        let other_text = run(vec![step(result("the bytes end early"), &[])]);
        assert!(judge(&input, &expected, &other_text).is_failure());
    }

    #[test]
    fn a_fixed_eviction_is_documented_when_the_pool_keeps_the_evicted() {
        let input = json!({"steps": [{"op": "process", "entries": [], "now": 0}]});
        let theirs = json!({"accept": [], "broadcast": [], "invalid": ["n"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "m"}},
            "notes": {"fullPoolEviction": true, "evictions": [{"id": "n", "evicted": ["e"]}]}});
        let ours = json!({"accept": [], "broadcast": [], "invalid": ["n"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "other"}}});
        let expected = run(vec![step(theirs, &["b"])]);
        assert_eq!(
            judge(
                &input,
                &expected,
                &run(vec![step(ours.clone(), &["b", "e"])])
            ),
            Outcome::Documented(Documented::FullPoolEviction.reason().into())
        );
        assert!(judge(&input, &expected, &run(vec![step(ours, &["b"])])).is_failure());
    }

    #[test]
    fn removed_types_are_left_out_of_the_lists() {
        let mut bytes = vec![0xff, 3, 90, 1, 0, 0, 0, 3, 0];
        bytes.extend([0; 40]);
        let hex_text = hex::encode(&bytes);
        let input = process(json!([{"hex": hex_text}, {"hex": "00"}]));
        let theirs = json!({"accept": ["t"], "broadcast": ["t"], "invalid": ["idx"], "excess": [],
            "errors": {"idx": {"type": "ERR_OTHER", "message": "x"}}});
        let sha = hex::encode(&sha256(&bytes));
        let ours = json!({"accept": ["t"], "broadcast": ["t"], "invalid": [sha], "excess": [],
            "errors": {sha: {"type": "ERR_OTHER", "message": "x"}}});
        let expected = run(vec![step(theirs.clone(), &["t"])]);
        // The reference's id of the entry is not one this side can name: a mismatch.
        assert!(judge(&input, &expected, &run(vec![step(ours.clone(), &["t"])])).is_failure());
        let theirs_indexed = json!({"accept": ["t"], "broadcast": ["t"], "invalid": ["0"], "excess": [],
            "errors": {"0": {"type": "ERR_BAD_DATA", "message": "Invalid transaction data: x"}}});
        assert_eq!(
            judge(
                &input,
                &run(vec![step(theirs_indexed, &["t"])]),
                &run(vec![step(ours, &["t"])])
            ),
            Outcome::Documented(Documented::RemovedType.reason().into())
        );
    }

    #[test]
    fn votes_with_indices_are_found_in_json_entries() {
        let input =
            process(json!([{"json": {"typeGroup": 2, "type": 2, "asset": {"votes": ["a"]}}}]));
        let found = documented_entries(&input["steps"][0]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, Form::VotesWithIndices);
        let plain =
            process(json!([{"json": {"typeGroup": 2, "type": 2, "asset": {"votes": {"a": 100}}}}]));
        assert!(documented_entries(&plain["steps"][0]).is_empty());
    }
}
