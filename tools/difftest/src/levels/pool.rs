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
//!   ([`Documented::FullPoolEviction`]): Heartwood refuses the same transaction without evicting,
//!   and the entries before it must have the same outcomes on both sides. From that entry on the
//!   two pools differ on purpose (the reference has a free place, Heartwood's pool is still full),
//!   so the rest of the step and the later steps are not compared. A step whose results and
//!   contents are equal all the same (a later entry evicted the same transaction on both sides) is
//!   compared as any other;
//! - entries of a removed type and JSON votes given as an array or a string
//!   ([`Documented::RemovedType`], [`Documented::VotesWithIndices`]), checked on the entries
//!   themselves: the step's lists and errors must be equal once those entries are left out on both
//!   sides, whatever ids they were given. The reference names such a JSON entry by the id of the
//!   transaction it rebuilt, which no other entry and no id of Heartwood's answer names; at most
//!   one such id per documented JSON entry is left out of the reference's side too;
//! - a count in a message that the oracle's default locale groups otherwise
//!   ([`Documented::Locale`], shown by the oracle's `locale` note).

use std::collections::BTreeSet;

use heartwood_crypto::Transaction;
use heartwood_crypto::crypto::hash::sha256;
use heartwood_crypto::identities::Address;
use heartwood_crypto::utils::hex;
use heartwood_pool::{Options, Pool};
use serde_json::{Map, Value, json};

use crate::compare;
use crate::report::Outcome;
use crate::subject::{Documented, NOTES};

/// The version the online record gives a delegate, as the reference's forger announces it.
pub const VERSION: &str = "4.3.1";

/// The options of a pool session: the reference's defaults, then `given` merged in key by key,
/// with the storage under `dir` and one worker.
pub fn options(given: &Value, dir: &std::path::Path) -> Result<Options, String> {
    let mut options = Options::defaults(&|_| None, dir, 2);
    options.storage = dir.join("pool.sqlite");
    options.worker_count = 1;
    let Some(given) = given.as_object() else {
        return if given.is_null() {
            Ok(options)
        } else {
            Err("options is not an object".into())
        };
    };
    let count = |value: &Value, key: &str| {
        value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| format!("options.{key} is not a count"))
    };
    for (key, value) in given {
        match key.as_str() {
            "maxTransactionsInPool" => options.max_transactions_in_pool = count(value, key)?,
            "maxTransactionsPerSender" => options.max_transactions_per_sender = count(value, key)?,
            "maxTransactionsPerRequest" => {
                options.max_transactions_per_request = count(value, key)?;
            }
            "maxTransactionBytes" => options.max_transaction_bytes = count(value, key)?,
            "maxTransactionAge" => {
                options.max_transaction_age = value
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or("options.maxTransactionAge is not a height")?;
            }
            "allowedSenders" => {
                options.allowed_senders = value
                    .as_array()
                    .ok_or("options.allowedSenders is not a list")?
                    .iter()
                    .map(|a| {
                        a.as_str()
                            .and_then(|text| Address::from_base58(text).ok())
                            .ok_or_else(|| format!("{a} is not an address"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "dynamicFees" => {
                let fees = value
                    .as_object()
                    .ok_or("options.dynamicFees is not an object")?;
                for (name, v) in fees {
                    let amount = || {
                        v.as_u64()
                            .ok_or_else(|| format!("dynamicFees.{name} is not an amount"))
                    };
                    match name.as_str() {
                        "enabled" => {
                            options.dynamic_fees.enabled =
                                v.as_bool().ok_or("dynamicFees.enabled is not a flag")?;
                        }
                        "minFeePool" => options.dynamic_fees.min_fee_pool = amount()?,
                        "minFeeBroadcast" => options.dynamic_fees.min_fee_broadcast = amount()?,
                        "addonBytes" => {
                            for (kind, bytes) in v
                                .as_object()
                                .ok_or("dynamicFees.addonBytes is not an object")?
                            {
                                let bytes = bytes
                                    .as_u64()
                                    .ok_or_else(|| format!("addonBytes.{kind} is not a count"))?;
                                options.dynamic_fees.addon_bytes.insert(kind.clone(), bytes);
                            }
                        }
                        other => return Err(format!("unknown dynamicFees option {other}")),
                    }
                }
            }
            "enabled" | "storage" | "workerPool" => {}
            other => return Err(format!("unknown pool option {other}")),
        }
    }
    Ok(options)
}

/// The pool's contents in the oracle's shape: `all`, `bySender`, `lowest`, `highest`, `size`,
/// `wallets` (each sender's view of its own wallet) and `stored`.
pub fn contents(pool: &Pool) -> Result<Value, String> {
    let ids =
        |list: Vec<&Transaction>| -> Vec<String> { list.iter().map(|t| t.id().to_hex()).collect() };
    let query = pool.query();
    let senders: Vec<_> = pool.mempool().senders().collect();
    let by_sender: Vec<Value> = senders
        .iter()
        .map(|s| {
            json!([
                s.address().to_base58(),
                s.from_earliest()
                    .map(|t| t.id().to_hex())
                    .collect::<Vec<_>>()
            ])
        })
        .collect();
    let wallets: Vec<Value> = senders
        .iter()
        .map(|s| {
            let (balance, nonce) = s.wallet().map_or((0, 0), |w| (w.balance(), w.nonce()));
            json!([
                s.address().to_base58(),
                balance.to_string(),
                nonce.to_string()
            ])
        })
        .collect();
    let stored: Vec<Value> = pool
        .stored()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|row| json!([row.id.to_hex(), row.height]))
        .collect();
    Ok(json!({
        "all": ids(query.all()),
        "bySender": by_sender,
        "lowest": ids(query.from_lowest_priority()),
        "highest": ids(query.from_highest_priority()),
        "size": pool.size(),
        "wallets": wallets,
        "stored": stored,
    }))
}

/// The note of Heartwood's answer to a `process` step that lists the transaction id of each entry
/// (null for one that does not decode), for the judge to find an entry by its transaction's id.
pub const ENTRY_IDS: &str = "entryIds";

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
/// each (its index, the SHA-256 of its bytes, and for JSON the id it gives) and whether it is
/// JSON.
fn documented_entries(step: &Value) -> Vec<(Form, BTreeSet<String>, bool)> {
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
            out.push((form, ids, entry.get("json").is_some()));
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

/// A processor result without the entries whose ids are in `ids`.
fn without_entries(result: &Value, ids: &BTreeSet<String>) -> Value {
    entries_where(result, |id| !ids.contains(id))
}

/// A processor result with only the list items and errors whose ids `keep` accepts.
fn entries_where(result: &Value, keep: impl Fn(&str) -> bool) -> Value {
    let mut out = result.clone();
    let Some(object) = out.as_object_mut() else {
        return out;
    };
    for list in LISTS {
        if let Some(Value::Array(items)) = object.get_mut(list) {
            items.retain(|v| v.as_str().is_none_or(&keep));
        }
    }
    if let Some(Value::Object(errors)) = object.get_mut("errors") {
        errors.retain(|id, _| keep(id));
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

/// The ids of the reference's result `expected` that neither an entry of `step` nor Heartwood's
/// result `actual` names: the ids of transactions the reference rebuilt from a JSON entry.
fn rebuilt_ids(step: &Value, expected: &Value, actual: &Value) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = entry_ids(step).into_iter().flatten().collect();
    known.extend(result_ids(actual));
    result_ids(expected)
        .into_iter()
        .filter(|id| !known.contains(id))
        .collect()
}

/// Every id a processor result lists or maps to an error.
fn result_ids(result: &Value) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = LISTS
        .iter()
        .flat_map(|list| result[*list].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    if let Some(errors) = result["errors"].as_object() {
        out.extend(errors.keys().cloned());
    }
    out
}

/// The ids either side may give each entry of a `process` step, in entry order: its index, the
/// SHA-256 of its bytes (the transaction id of canonical bytes), and for JSON the id it gives.
fn entry_ids(step: &Value) -> Vec<BTreeSet<String>> {
    let Some(entries) = step.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let mut ids = BTreeSet::from([i.to_string()]);
            if let Some(text) = entry.get("hex").and_then(Value::as_str) {
                ids.insert(hex::encode(&sha256(&hex::decode(text).unwrap_or_default())));
            } else if let Some(id) = entry.pointer("/json/id").and_then(Value::as_str) {
                ids.insert(id.to_owned());
            }
            ids
        })
        .collect()
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

/// Whether Heartwood's step shows the fixed eviction the oracle noted: Heartwood refused every
/// transaction the reference refused after an eviction, and the entries before the first of them
/// have the same outcomes on both sides (the messages within the decode-reason rule).
fn eviction_holds(
    step: &Value,
    notes: &Value,
    ours: &Value,
    expected: &Value,
    actual: &Value,
) -> bool {
    let refused: Vec<&str> = notes
        .get("evictions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    let Some(first) = refused.first() else {
        return false;
    };
    let ours_invalid: BTreeSet<&str> = actual["invalid"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !refused.iter().all(|id| ours_invalid.contains(id)) {
        return false;
    }
    let mut entries = entry_ids(step);
    for (ids, id) in entries.iter_mut().zip(
        ours[ENTRY_IDS]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    ) {
        if let Some(id) = id.as_str() {
            ids.insert(id.to_owned());
        }
    }
    // An entry the reference refused that neither side can name leaves nothing to compare before it.
    let position = entries
        .iter()
        .position(|ids| ids.contains(*first))
        .unwrap_or(0);
    let before: BTreeSet<String> = entries.iter().take(position).flatten().cloned().collect();
    let (a, b) = (
        entries_where(expected, |id| before.contains(id)),
        entries_where(actual, |id| before.contains(id)),
    );
    compare::diff(&without_messages(&a), &without_messages(&b)).is_empty()
        && messages(&a, &b) != Messages::Different
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
        let mut b_result = b["result"].clone();
        let ours = b_result
            .as_object_mut()
            .and_then(|o| o.shift_remove(NOTES))
            .unwrap_or(Value::Null);
        let agree = || {
            compare::diff(&a_result, &b_result).is_empty()
                && compare::diff(&a["contents"], &b["contents"]).is_empty()
        };
        if notes["fullPoolEviction"] == Value::Bool(true) && !agree() {
            return if eviction_holds(step, &notes, &ours, &a_result, &b_result) {
                Outcome::Documented(Documented::FullPoolEviction.reason().to_owned())
            } else {
                Outcome::Mismatch(prefixed(
                    i,
                    compare::diff(
                        &json!([{"result": a_result, "contents": a["contents"]}]),
                        &json!([{"result": b_result, "contents": b["contents"]}]),
                    ),
                ))
            };
        }
        let contents = compare::diff(&a["contents"], &b["contents"]);
        let b_result = &b_result;
        let same_shape = compare::diff(&without_messages(&a_result), &without_messages(b_result));
        let (same_shape, forms) = if same_shape.is_empty() {
            (true, Vec::new())
        } else {
            let entries = documented_entries(step);
            let ids: BTreeSet<String> =
                entries.iter().flat_map(|(_, ids, _)| ids.clone()).collect();
            let rebuilt = rebuilt_ids(step, &a_result, b_result);
            let json = entries.iter().filter(|(_, _, json)| *json).count();
            let mut theirs = ids.clone();
            if rebuilt.len() <= json {
                theirs.extend(rebuilt);
            }
            let equal = !entries.is_empty()
                && compare::diff(
                    &without_messages(&without_entries(&a_result, &theirs)),
                    &without_messages(&without_entries(b_result, &ids)),
                )
                .is_empty();
            if !equal {
                return Outcome::Mismatch(prefixed(i, same_shape));
            }
            (
                false,
                entries.into_iter().map(|(form, _, _)| form).collect(),
            )
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
    fn a_fixed_eviction_is_documented_when_the_entries_before_it_agree() {
        // Entry 0 is accepted by both; entry 1 (id n) made the reference evict e and was refused;
        // entry 2 then took the reference's free place, while Heartwood's pool is still full.
        let input =
            process(json!([{"json": {"id": "a"}}, {"json": {"id": "n"}}, {"json": {"id": "c"}}]));
        let theirs = json!({"accept": ["a", "c"], "broadcast": ["a", "c"], "invalid": ["n"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "m"}},
            "notes": {"fullPoolEviction": true, "evictions": [{"id": "n", "evicted": ["e"]}]}});
        let ours = json!({"accept": ["a"], "broadcast": ["a"], "invalid": ["n", "c"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "other"},
                       "c": {"type": "ERR_POOL_FULL", "message": "full"}}});
        let expected = run(vec![step(theirs, &["a", "c"])]);
        assert_eq!(
            judge(
                &input,
                &expected,
                &run(vec![step(ours.clone(), &["a", "e"])])
            ),
            Outcome::Documented(Documented::FullPoolEviction.reason().into())
        );
        // Heartwood must refuse the transaction the reference refused after its eviction.
        let mut admitted = ours.clone();
        admitted["accept"] = json!(["a", "n"]);
        admitted["invalid"] = json!(["c"]);
        assert!(judge(&input, &expected, &run(vec![step(admitted, &["a", "n"])])).is_failure());
        // The entries before it must agree.
        let mut earlier = ours;
        earlier["accept"] = json!([]);
        earlier["broadcast"] = json!([]);
        assert!(judge(&input, &expected, &run(vec![step(earlier, &["e"])])).is_failure());
    }

    #[test]
    fn heartwood_s_entry_ids_find_an_entry_whose_json_id_is_wrong() {
        // The entries give no id the reference uses; Heartwood's note names each transaction.
        let input = process(json!([{"json": {"id": "x"}}, {"json": {}}, {"json": {"id": "y"}}]));
        let theirs = json!({"accept": ["a", "c"], "broadcast": ["a", "c"], "invalid": ["n"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "m"}},
            "notes": {"fullPoolEviction": true, "evictions": [{"id": "n", "evicted": ["e"]}]}});
        let ours = |accept: Value| {
            json!({"accept": accept, "broadcast": accept, "invalid": ["n", "c"], "excess": [],
                "errors": {"n": {"type": "ERR_APPLY", "message": "m"},
                           "c": {"type": "ERR_POOL_FULL", "message": "full"}},
                "notes": {ENTRY_IDS: ["a", "n", "c"]}})
        };
        let expected = run(vec![step(theirs, &["a", "c"])]);
        assert_eq!(
            judge(
                &input,
                &expected,
                &run(vec![step(ours(json!(["a"])), &["a", "e"])])
            ),
            Outcome::Documented(Documented::FullPoolEviction.reason().into())
        );
        // Entry 0 comes before the eviction: it must agree.
        assert!(judge(&input, &expected, &run(vec![step(ours(json!([])), &["e"])])).is_failure());
    }

    #[test]
    fn a_step_that_agrees_after_a_noted_eviction_is_compared() {
        // A later entry evicted the same transaction on both sides: the step is compared as any
        // other, and the next step too.
        let input = json!({"steps": [{"op": "process", "entries": [], "now": 0},
                                     {"op": "process", "entries": [], "now": 0}]});
        let result = json!({"accept": ["m"], "broadcast": ["m"], "invalid": ["n"], "excess": [],
            "errors": {"n": {"type": "ERR_APPLY", "message": "m"}}});
        let mut noted = result.clone();
        noted["notes"] =
            json!({"fullPoolEviction": true, "evictions": [{"id": "n", "evicted": ["e"]}]});
        let next = json!({"accept": [], "broadcast": [], "invalid": [], "excess": []});
        let expected = run(vec![
            step(noted, &["b", "m"]),
            step(next.clone(), &["b", "m"]),
        ]);
        assert_eq!(
            judge(
                &input,
                &expected,
                &run(vec![
                    step(result.clone(), &["b", "m"]),
                    step(next.clone(), &["b", "m"])
                ])
            ),
            Outcome::Pass
        );
        assert!(
            judge(
                &input,
                &expected,
                &run(vec![step(result, &["b", "m"]), step(next, &["b"])])
            )
            .is_failure()
        );
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
    fn a_rebuilt_json_vote_is_left_out_under_the_reference_s_id() {
        let vote = |votes: Value| json!({"json": {"typeGroup": 2, "type": 2, "id": "g", "asset": {"votes": votes}}});
        let input = process(json!([vote(json!(["a"])), vote(json!("a")), {"hex": "00"}]));
        let bad = |message: &str| json!({"type": "ERR_BAD_DATA", "message": message});
        let theirs = json!({"accept": ["t"], "broadcast": ["t"], "invalid": ["r1", "r2"], "excess": [],
            "errors": {"r1": bad("r1 failed signature verification check"),
                       "r2": bad("r2 failed signature verification check")}});
        let ours = json!({"accept": ["t"], "broadcast": ["t"], "invalid": ["g", "g"], "excess": [],
            "errors": {"g": bad("Invalid transaction data: votes")}});
        assert_eq!(
            judge(
                &input,
                &run(vec![step(theirs.clone(), &["t"])]),
                &run(vec![step(ours.clone(), &["t"])])
            ),
            Outcome::Documented(Documented::VotesWithIndices.reason().into())
        );
        // More ids than documented JSON entries: one of them belongs to another entry.
        let mut three = theirs;
        three["invalid"] = json!(["r1", "r2", "r3"]);
        three["errors"]["r3"] = bad("r3");
        assert!(
            judge(
                &input,
                &run(vec![step(three, &["t"])]),
                &run(vec![step(ours, &["t"])])
            )
            .is_failure()
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
