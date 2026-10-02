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
//!   compared as any other. A re-add (`readd`, or the re-add of a `restart`) evicts in the same
//!   way and is noted the same way, with the re-add's attempts (each transaction it tried to add
//!   back, in order, with whether it entered the pool), and Heartwood's answer lists its own
//!   ([`READD_ATTEMPTS`]): they must agree up to and including the reference's first refusal after
//!   an eviction, which Heartwood refused without evicting. With no result per transaction, the
//!   pool's contents show the rest: Heartwood's pool holds a transaction the reference refused only
//!   where the reference's pool holds it too, every transaction either pool holds was tried in the
//!   re-add, every transaction only Heartwood's pool holds is one the reference evicted or a later
//!   one of its sender, and what the evictions do not touch (each pool's size, and the other
//!   senders' transactions, their priority order, wallets and stored rows) is equal. The later
//!   steps are not compared;
//! - entries of a removed type and JSON votes given as an array or a string
//!   ([`Documented::RemovedType`], [`Documented::VotesWithIndices`]), checked on the entries
//!   themselves: the step's lists and errors, with their messages under the rules here, must be
//!   equal once those entries are left out on both sides, whatever ids they were given. The reference names such a JSON entry by the id of the
//!   transaction it rebuilt, which no other entry and no id of Heartwood's answer names; at most
//!   one such id per documented JSON entry is left out of the reference's side too;
//! - a count in a message that the oracle's default locale groups otherwise
//!   ([`Documented::Locale`], shown by the oracle's `locale` note);
//! - after the session changed the online record, a step whose entries differ only where one
//!   side refused a vote or revoke as offline ([`Documented::OnlineView`]): the reference's
//!   sender view keeps the delegate's version it copied before the change. The entries of a
//!   documented form are left out first, as in any other step, and must be refused on both sides;
//!   decode reasons are allowed. The later steps are not compared, since the two pools may differ
//!   from there.

use std::collections::{BTreeMap, BTreeSet};

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

/// The note of Heartwood's answer to a re-add (`readd`, `restart`): each transaction the re-add
/// tried to add back, in its order, as `[id, entered]`, for the judge to check an eviction the
/// oracle notes at a re-add.
pub const READD_ATTEMPTS: &str = "readdAttempts";

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

/// The end of the pool-entry message of a vote or revoke for a delegate that the online record
/// does not hold.
const OFFLINE: &str = "is not operating a node on the network";

/// Whether every entry whose outcome differs between the two processor results was refused as
/// offline on one side (and at least one differs). The reason of an entry that does not decode is
/// left out, as the decode-reason rule does.
fn only_offline_differs(expected: &Value, actual: &Value) -> bool {
    let outcome = |result: &Value, id: &str| {
        let mut error = result["errors"][id].clone();
        let decode = error["type"] == "ERR_BAD_DATA"
            && error["message"]
                .as_str()
                .is_some_and(|m| m.starts_with(DECODE_PREFIX));
        if decode {
            error["message"] = Value::from(DECODE_PREFIX);
        }
        let listed: Vec<bool> = LISTS
            .iter()
            .map(|list| {
                result[*list]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id)))
            })
            .collect();
        json!([listed, error])
    };
    let offline = |result: &Value, id: &str| {
        result["errors"][id]["message"]
            .as_str()
            .is_some_and(|m| m.ends_with(OFFLINE))
    };
    let mut ids = result_ids(expected);
    ids.extend(result_ids(actual));
    let differing: Vec<&String> = ids
        .iter()
        .filter(|id| outcome(expected, id) != outcome(actual, id))
        .collect();
    !differing.is_empty()
        && differing
            .iter()
            .all(|id| offline(expected, id) || offline(actual, id))
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
/// have the same outcomes on both sides (the messages within the decode-reason rule), apart from
/// the entries of a documented form, which are left out as in any other step.
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
    let documented: BTreeSet<String> = documented_entries(step)
        .into_iter()
        .flat_map(|(_, ids, _)| ids)
        .collect();
    let before: BTreeSet<String> = entries
        .iter()
        .take(position)
        .filter(|ids| ids.is_disjoint(&documented))
        .flatten()
        .cloned()
        .collect();
    let (a, b) = (
        entries_where(expected, |id| before.contains(id)),
        entries_where(actual, |id| before.contains(id)),
    );
    compare::diff(&without_messages(&a), &without_messages(&b)).is_empty()
        && messages(&a, &b) != Messages::Different
}

/// One attempt of a re-add: the transaction id, whether it entered the pool, and, on the
/// reference's side, whether its refusal came after the full pool evicted a transaction.
type Attempt<'a> = (&'a str, bool, bool);

/// The attempts of a re-add's note, or `None` when one is malformed.
fn attempts(note: &Value) -> Option<Vec<Attempt<'_>>> {
    note.as_array()?
        .iter()
        .map(|a| {
            Some((
                a.get(0)?.as_str()?,
                a.get(1)?.as_bool()?,
                a.get(2) == Some(&Value::Bool(true)),
            ))
        })
        .collect()
}

/// Whether Heartwood's re-add (a `readd` or `restart` step) shows the fixed eviction the oracle
/// noted in `notes`, given Heartwood's notes of the step (`ours`) and the pool's contents after the
/// step on the reference's side (`expected`) and on Heartwood's (`actual`):
///
/// - the two re-adds tried the same transactions in the same order with the same outcomes up to
///   and including the reference's first refusal after an eviction, which Heartwood refused too,
///   without evicting ([`READD_ATTEMPTS`]);
/// - the contents differ only as that eviction makes them differ ([`readd_contents_hold`]), the
///   transactions tried before that refusal and from it on taken from the reference's attempts.
///
/// From there on the two pools differ on purpose, so the later steps are not compared.
fn readd_eviction_holds(notes: &Value, ours: &Value, expected: &Value, actual: &Value) -> bool {
    let (Some(theirs), Some(ours)) = (
        attempts(&notes["attempts"]),
        attempts(&ours[READD_ATTEMPTS]),
    ) else {
        return false;
    };
    let Some(first) = theirs.iter().position(|(_, _, evicted)| *evicted) else {
        return false;
    };
    let agree = ours.len() > first
        && (theirs.iter().zip(&ours))
            .take(first + 1)
            .all(|((a, a_entered, _), (b, b_entered, _))| a == b && a_entered == b_entered);
    let tried = |list: &[Attempt<'_>]| -> Vec<String> {
        list.iter().map(|(id, _, _)| (*id).to_owned()).collect()
    };
    agree
        && readd_contents_hold(
            notes,
            expected,
            actual,
            &tried(&theirs[..first]),
            &tried(&theirs[first..]),
        )
}

/// Whether the pool's contents after a re-add, the reference's (`expected`) and Heartwood's
/// (`actual`), differ only as the evictions noted in `notes` make them differ, given the
/// transactions the reference tried `before` its first refusal after an eviction and from that
/// refusal on (`from`):
///
/// - every transaction either pool holds was tried in the re-add, which empties the mempool first;
/// - Heartwood holds a transaction the reference refused after an eviction only where the
///   reference's pool holds it too, as an earlier copy;
/// - every transaction only Heartwood's pool holds is one the reference evicted, or a later
///   transaction of that sender, which the reference then refused for the nonce its evicted
///   predecessor left;
/// - no transaction tried before the first refusal is held by the reference's pool only: up to it
///   both pools went through the same states, and from it on Heartwood's pool holds the evicted
///   transaction where the reference has a free place, until a later admission evicts it;
/// - a transaction the reference evicted is missing from Heartwood's pool only where it was tried
///   after the first refusal (Heartwood, still full, may have refused it) or Heartwood holds one
///   tried after it (Heartwood evicts it only to admit a later one);
/// - the rest of the contents, which the evictions leave alone, agree ([`untouched_agree`]).
fn readd_contents_hold(
    notes: &Value,
    expected: &Value,
    actual: &Value,
    before: &[String],
    from: &[String],
) -> bool {
    let after = from.get(1..).unwrap_or_default();
    let evictions = notes
        .get("evictions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let refused: BTreeSet<&str> = evictions
        .iter()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    let evicted: BTreeSet<&str> = evictions
        .iter()
        .flat_map(|e| {
            e.get("evicted")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(Value::as_str)
        .collect();
    if refused.is_empty() || evicted.is_empty() {
        return false;
    }
    let held =
        |contents: &Value| -> BTreeSet<String> { listed(&contents["all"]).into_iter().collect() };
    let (theirs, ours) = (held(expected), held(actual));
    let tried: BTreeSet<&String> = before.iter().chain(from).collect();
    if !theirs.iter().chain(&ours).all(|id| tried.contains(id)) {
        return false;
    }
    if refused
        .iter()
        .any(|id| ours.contains(*id) && !theirs.contains(*id))
    {
        return false;
    }
    // Each sender's transactions in Heartwood's pool, oldest first: from an evicted one on.
    let mut kept: BTreeSet<String> = BTreeSet::new();
    for sender in actual["bySender"].as_array().into_iter().flatten() {
        let ids = listed(&sender[1]);
        if let Some(at) = ids.iter().position(|id| evicted.contains(id.as_str())) {
            kept.extend(ids[at..].iter().cloned());
        }
    }
    if !ours.difference(&theirs).all(|id| kept.contains(id)) {
        return false;
    }
    let tried_after = |id: &str| after.iter().any(|a| a == id);
    let admitted_after = after.iter().any(|id| ours.contains(id));
    if evicted
        .iter()
        .any(|id| !ours.contains(*id) && !tried_after(id) && !admitted_after)
    {
        return false;
    }
    if before
        .iter()
        .any(|id| theirs.contains(id) && !ours.contains(id))
    {
        return false;
    }
    let exempt: BTreeSet<String> = (evicted.iter().chain(&refused))
        .map(|id| (*id).to_owned())
        .chain(after.iter().cloned())
        .chain(kept)
        .collect();
    untouched_agree(expected, actual, &exempt)
}

/// The ids of `list`, a JSON array of ids.
fn listed(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Whether the parts of two pools' contents after a re-add that its evictions leave alone agree,
/// `exempt` being the transactions the evictions concern (the evicted ones and their senders'
/// later ones, the refused ones, and those tried from the first refusal on), and the senders
/// holding any of them on either side being the senders the evictions concern:
///
/// - each pool's size is the number of its transactions, and its priority orders (`highest`,
///   `lowest`) hold each of its transactions once;
/// - the other senders have the same transactions, the same wallet and, apart from `exempt`, the
///   same stored rows on both sides, and their transactions come in the same order in `highest`:
///   the priority order of two senders' transactions depends on those two senders' transactions
///   and their order in the pool only, which the evictions leave alone (a sender they concern may
///   come back later in that order, after its only transaction was evicted).
fn untouched_agree(expected: &Value, actual: &Value, exempt: &BTreeSet<String>) -> bool {
    let consistent = |contents: &Value| {
        let mut all = listed(&contents["all"]);
        all.sort();
        let views = ["highest", "lowest"].iter().all(|view| {
            let mut list = listed(&contents[*view]);
            list.sort();
            list == all
        });
        let unique = all.windows(2).all(|pair| pair[0] != pair[1]);
        let sized = contents["size"]
            .as_u64()
            .is_some_and(|size| usize::try_from(size).is_ok_and(|size| size == all.len()));
        views && unique && sized
    };
    if !consistent(expected) || !consistent(actual) {
        return false;
    }
    let senders = |contents: &Value| -> BTreeMap<String, Vec<String>> {
        (contents["bySender"].as_array().into_iter().flatten())
            .filter_map(|sender| Some((sender[0].as_str()?.to_owned(), listed(&sender[1]))))
            .collect()
    };
    let (theirs, ours) = (senders(expected), senders(actual));
    let involved: BTreeSet<String> = (theirs.iter().chain(&ours))
        .filter(|(_, ids)| ids.iter().any(|id| exempt.contains(id)))
        .map(|(address, _)| address.clone())
        .collect();
    let apart = |map: &BTreeMap<String, Vec<String>>| -> BTreeMap<String, Vec<String>> {
        map.iter()
            .filter(|(address, _)| !involved.contains(*address))
            .map(|(address, ids)| (address.clone(), ids.clone()))
            .collect()
    };
    let (their_others, our_others) = (apart(&theirs), apart(&ours));
    if their_others != our_others {
        return false;
    }
    let others: BTreeSet<&String> = their_others.values().flatten().collect();
    let in_order = |contents: &Value| -> Vec<String> {
        listed(&contents["highest"])
            .into_iter()
            .filter(|id| others.contains(id))
            .collect()
    };
    if in_order(expected) != in_order(actual) {
        return false;
    }
    let wallets = |contents: &Value| -> BTreeMap<String, Value> {
        (contents["wallets"].as_array().into_iter().flatten())
            .filter_map(|wallet| Some((wallet[0].as_str()?.to_owned(), wallet.clone())))
            .filter(|(address, _)| !involved.contains(address))
            .collect()
    };
    let stored = |contents: &Value| -> Vec<Value> {
        (contents["stored"].as_array().into_iter().flatten())
            .filter(|row| row[0].as_str().is_some_and(|id| !exempt.contains(id)))
            .cloned()
            .collect()
    };
    wallets(expected) == wallets(actual) && stored(expected) == stored(actual)
}

/// Whether neither result admitted an entry of a documented form: none of `theirs` is in the
/// reference's `accept` or `broadcast` list, and none of `ours` in Heartwood's.
fn documented_refused(
    expected: &Value,
    theirs: &BTreeSet<String>,
    actual: &Value,
    ours: &BTreeSet<String>,
) -> bool {
    let admitted = |result: &Value, ids: &BTreeSet<String>| {
        ["accept", "broadcast"].iter().any(|list| {
            result[*list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|id| ids.contains(id))
        })
    };
    !admitted(expected, theirs) && !admitted(actual, ours)
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
    let online_view = |i: usize, a: &Value, b: &Value| {
        steps.get(..i).is_some_and(|before| {
            before
                .iter()
                .any(|s| s.get("op").and_then(Value::as_str) == Some("setOnline"))
        }) && only_offline_differs(a, b)
    };
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
            let holds = match step.get("op").and_then(Value::as_str) {
                Some("readd" | "restart") => {
                    compare::diff(&a_result, &b_result).is_empty()
                        && readd_eviction_holds(&notes, &ours, &a["contents"], &b["contents"])
                }
                _ => eviction_holds(step, &notes, &ours, &a_result, &b_result),
            };
            return if holds {
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
        // The two results whose messages are compared: the whole results, or, when entries of a
        // documented form make them differ, the results without those entries; and the ids each
        // side's documented entries were left out under (none when the shapes agree).
        let (a_left, b_left, forms, left_out) = if same_shape.is_empty() {
            let none = (BTreeSet::new(), BTreeSet::new());
            (a_result.clone(), b_result.clone(), Vec::new(), none)
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
            let (a_left, b_left) = (
                without_entries(&a_result, &theirs),
                without_entries(b_result, &ids),
            );
            let equal = !entries.is_empty()
                && compare::diff(&without_messages(&a_left), &without_messages(&b_left)).is_empty();
            if !equal {
                // An offline refusal is judged beside the documented entries, left out as here
                // once both sides have refused them.
                return if online_view(i, &a_left, &b_left)
                    && documented_refused(&a_result, &theirs, b_result, &ids)
                {
                    Outcome::Documented(Documented::OnlineView.reason().to_owned())
                } else {
                    Outcome::Mismatch(prefixed(i, same_shape))
                };
            }
            let forms: Vec<Form> = entries.into_iter().map(|(form, _, _)| form).collect();
            (a_left, b_left, forms, (theirs, ids))
        };
        if !contents.is_empty() {
            return Outcome::Mismatch(prefixed(i, contents));
        }
        if !forms.is_empty() {
            documented.get_or_insert(if forms.contains(&Form::RemovedType) {
                Documented::RemovedType
            } else {
                Documented::VotesWithIndices
            });
        }
        match messages(&a_left, &b_left) {
            Messages::Equal => {}
            Messages::DecodeReason => {
                documented.get_or_insert(Documented::DecodeReason);
            }
            Messages::Different if locale => {
                documented.get_or_insert(Documented::Locale);
            }
            Messages::Different
                if online_view(i, &a_left, &b_left)
                    && documented_refused(&a_result, &left_out.0, b_result, &left_out.1) =>
            {
                return Outcome::Documented(Documented::OnlineView.reason().to_owned());
            }
            Messages::Different => {
                return Outcome::Mismatch(prefixed(i, compare::diff(&a_left, &b_left)));
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
    fn a_removed_type_before_a_noted_eviction_is_left_out() {
        let mut bytes = vec![0xff, 3, 90, 1, 0, 0, 0, 9, 0];
        bytes.extend([0; 40]);
        let sha = hex::encode(&sha256(&bytes));
        let input = process(
            json!([{"json": {"id": "a"}}, {"hex": hex::encode(&bytes)}, {"json": {"id": "n"}}]),
        );
        let bad = json!({"type": "ERR_BAD_DATA", "message": "Invalid transaction data: x"});
        let theirs = json!({"accept": ["a"], "broadcast": ["a"], "invalid": ["1", "n"], "excess": [],
            "errors": {"1": bad, "n": {"type": "ERR_APPLY", "message": "m"}},
            "notes": {"fullPoolEviction": true, "evictions": [{"id": "n", "evicted": ["a"]}]}});
        let ours = json!({"accept": ["a"], "broadcast": ["a"], "invalid": [sha, "n"], "excess": [],
            "errors": {sha: {"type": "ERR_OTHER", "message": "Transaction type Core/9 is deactivated"},
                       "n": {"type": "ERR_APPLY", "message": "m"}}});
        assert_eq!(
            judge(
                &input,
                &run(vec![step(theirs, &[])]),
                &run(vec![step(ours, &["a"])])
            ),
            Outcome::Documented(Documented::FullPoolEviction.reason().into())
        );
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
    fn the_other_entries_messages_are_compared_beside_a_documented_entry() {
        let removed = json!({"json": {"typeGroup": 1, "type": 0, "id": null}});
        let input = process(json!([removed, {"hex": "00"}]));
        let deactivated =
            json!({"type": "ERR_OTHER", "message": "Transaction type Core/0 is deactivated"});
        let applied = |message: &str| json!({"type": "ERR_APPLY", "message": message});
        // The reference names the removed type by the id it computes, Heartwood by its index.
        let theirs = json!({"accept": [], "broadcast": [], "invalid": ["c", "x"], "excess": [],
            "errors": {"c": deactivated, "x": applied("x cannot be applied: one")}});
        let ours = |message: &str| {
            json!({"accept": [], "broadcast": [], "invalid": ["0", "x"], "excess": [],
                   "errors": {"0": deactivated, "x": applied(message)}})
        };
        let expected = run(vec![step(theirs, &[])]);
        assert_eq!(
            judge(
                &input,
                &expected,
                &run(vec![step(ours("x cannot be applied: one"), &[])])
            ),
            Outcome::Documented(Documented::RemovedType.reason().into())
        );
        assert!(
            judge(
                &input,
                &expected,
                &run(vec![step(ours("x cannot be applied: two"), &[])])
            )
            .is_failure()
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
    fn an_offline_refusal_after_a_change_of_the_online_record_is_documented() {
        let set_online = json!({"op": "setOnline", "delegates": [], "expireAt": 9});
        let batch = json!({"op": "process", "entries": [{"hex": "00"}], "now": 0});
        let refused = |message: &str| {
            json!({"accept": [], "broadcast": [], "invalid": ["r"], "excess": [],
                   "errors": {"r": {"type": "ERR_APPLY", "message": message}}})
        };
        let theirs = refused("r cannot be applied: the wallet has not resigned");
        let ours = refused("r cannot be applied: d0 is not operating a node on the network");
        let session = |steps: Vec<Value>| json!({ "steps": steps });
        let answer = |result: Value| run(vec![step(json!({"online": []}), &[]), step(result, &[])]);
        assert_eq!(
            judge(
                &session(vec![set_online.clone(), batch.clone()]),
                &answer(theirs.clone()),
                &answer(ours.clone())
            ),
            Outcome::Documented(Documented::OnlineView.reason().into())
        );
        // Without a change of the record first, a mismatch.
        let other = json!({"op": "flush"});
        assert!(
            judge(
                &session(vec![other, batch.clone()]),
                &answer(theirs.clone()),
                &answer(ours)
            )
            .is_failure()
        );
        // A difference that is not an offline refusal is a mismatch.
        assert!(
            judge(
                &session(vec![set_online, batch]),
                &answer(theirs),
                &answer(refused("r cannot be applied: another reason"))
            )
            .is_failure()
        );
    }

    #[test]
    fn an_offline_refusal_beside_a_removed_type_is_documented() {
        // A removed type as bytes, which the reference names by its index and Heartwood by its
        // hash, beside a vote that one side refused as offline.
        let mut bytes = vec![0xff, 3, 90, 1, 0, 0, 0, 3, 0];
        bytes.extend([0; 40]);
        let sha = hex::encode(&sha256(&bytes));
        let set_online = json!({"op": "setOnline", "delegates": [], "expireAt": 9});
        let batch = json!({"op": "process", "now": 0,
                           "entries": [{"hex": hex::encode(&bytes)}, {"hex": "00"}]});
        let decoded = json!({"type": "ERR_BAD_DATA", "message": "Invalid transaction data: x"});
        let removed =
            json!({"type": "ERR_OTHER", "message": "Transaction type Core/3 is deactivated"});
        let refused = |removed: (&str, &Value), message: &str| {
            let applied = json!({"type": "ERR_APPLY", "message": message});
            json!({"accept": [], "broadcast": [], "invalid": [removed.0, "r"], "excess": [],
                   "errors": {removed.0: removed.1, "r": applied}})
        };
        let theirs = |message: &str| refused(("0", &decoded), message);
        let ours = |message: &str| refused((sha.as_str(), &removed), message);
        let offline = |name: &str| {
            format!("r cannot be applied: {name} is not operating a node on the network")
        };
        let nonce =
            "r cannot be applied: Cannot apply a transaction with nonce 7: the sender has nonce 2";
        let session = |first: &Value| json!({"steps": [first, batch]});
        let answer = |result: Value| run(vec![step(json!({"online": []}), &[]), step(result, &[])]);
        let online_view = Outcome::Documented(Documented::OnlineView.reason().into());
        // The reference's view kept the delegate's version; Heartwood refused the vote as offline.
        assert_eq!(
            judge(
                &session(&set_online),
                &answer(theirs(nonce)),
                &answer(ours(&offline("d2")))
            ),
            online_view
        );
        // The other way round.
        assert_eq!(
            judge(
                &session(&set_online),
                &answer(theirs(&offline("d2"))),
                &answer(ours(nonce))
            ),
            online_view
        );
        // Both refused it as offline, naming the first delegate each saw offline.
        assert_eq!(
            judge(
                &session(&set_online),
                &answer(theirs(&offline("d2"))),
                &answer(ours(&offline("d1")))
            ),
            online_view
        );
        // Without a change of the online record first, a mismatch.
        let flush = json!({"op": "flush"});
        assert!(
            judge(
                &session(&flush),
                &answer(theirs(nonce)),
                &answer(ours(&offline("d2")))
            )
            .is_failure()
        );
        // The reference admitted the vote (its view still held the version), Heartwood refused
        // it as offline; the removed type was refused on both sides.
        let admitted_vote = json!({"accept": ["r"], "broadcast": ["r"], "invalid": ["0"],
                                   "excess": [], "errors": {"0": decoded}});
        assert_eq!(
            judge(
                &session(&set_online),
                &answer(admitted_vote.clone()),
                &answer(ours(&offline("d2")))
            ),
            online_view
        );
        // The same, but Heartwood admitted the removed type: a mismatch.
        let admitted_removed = json!({"accept": [sha], "broadcast": [sha], "invalid": ["r"],
                                      "excess": [], "errors": {"r": {"type": "ERR_APPLY",
                                      "message": offline("d2")}}});
        assert!(
            judge(
                &session(&set_online),
                &answer(admitted_vote),
                &answer(admitted_removed)
            )
            .is_failure()
        );
        // A difference beside the removed type that is no offline refusal is a mismatch.
        assert!(
            judge(
                &session(&set_online),
                &answer(theirs(nonce)),
                &answer(ours("r cannot be applied: another reason"))
            )
            .is_failure()
        );
        // A JSON vote that the reference admitted and Heartwood refused as offline, beside a
        // removed type as bytes that both refused: documented; refused for another reason, or with
        // the removed type admitted by Heartwood, a mismatch.
        let mut bytes = vec![0xff, 3, 90, 1, 0, 0, 0, 9, 0];
        bytes.extend([0; 40]);
        let sha = hex::encode(&sha256(&bytes));
        let batch = json!({"op": "process", "now": 0,
                           "entries": [{"hex": hex::encode(&bytes)}, {"json": {"id": "v"}}]});
        let session = json!({"steps": [set_online, batch]});
        let admitted_v = json!({"accept": ["v"], "broadcast": ["v"], "invalid": ["0"],
                                "excess": [], "errors": {"0": decoded}});
        let removed =
            json!({"type": "ERR_OTHER", "message": "Transaction type Core/9 is deactivated"});
        let refused_v = |message: &str| {
            json!({"accept": [], "broadcast": [], "invalid": [sha.clone(), "v"], "excess": [],
                   "errors": {sha.clone(): removed.clone(),
                              "v": {"type": "ERR_APPLY", "message": message}}})
        };
        let answer = |result: Value, all: &[&str]| {
            run(vec![step(json!({"online": []}), &[]), step(result, all)])
        };
        let offline_v = "v cannot be applied: d0 is not operating a node on the network";
        assert_eq!(
            judge(
                &session,
                &answer(admitted_v.clone(), &["v"]),
                &answer(refused_v(offline_v), &[])
            ),
            online_view
        );
        assert!(
            judge(
                &session,
                &answer(admitted_v.clone(), &["v"]),
                &answer(refused_v("v cannot be applied: another reason"), &[])
            )
            .is_failure()
        );
        // The shapes agree once a removed type is left out, and the rest differs only by an
        // offline refusal: still a mismatch where Heartwood admitted the removed type, and the
        // online view where both sides refused it.
        let json_batch = json!({"op": "process", "now": 0, "entries": [
            {"json": {"id": "m", "typeGroup": 1, "type": 3}}, {"json": {"id": "w"}}]});
        let json_session = json!({"steps": [set_online, json_batch]});
        let removed_m = json!({"type": "ERR_OTHER", "message": "m: Core/3 is deactivated"});
        let other_w =
            json!({"type": "ERR_APPLY", "message": "w cannot be applied: another reason"});
        let offline_w = json!({"type": "ERR_APPLY",
                               "message": "w cannot be applied: d0 is not operating a node on the network"});
        let both_refused = json!({"accept": [], "broadcast": [], "invalid": ["m", "w"], "excess": [],
                                  "errors": {"m": removed_m, "w": other_w}});
        let m_admitted = json!({"accept": ["m"], "broadcast": ["m"], "invalid": ["w"], "excess": [],
                                "errors": {"w": offline_w}});
        assert!(
            judge(
                &json_session,
                &answer(both_refused.clone(), &[]),
                &answer(m_admitted, &[])
            )
            .is_failure()
        );
        let m_refused = json!({"accept": [], "broadcast": [], "invalid": ["m", "w"], "excess": [],
                               "errors": {"m": removed_m, "w": offline_w}});
        assert_eq!(
            judge(
                &json_session,
                &answer(both_refused, &[]),
                &answer(m_refused, &[])
            ),
            online_view
        );
        let mut admitted_sha = refused_v(offline_v);
        admitted_sha["accept"] = json!([sha.clone()]);
        admitted_sha["broadcast"] = json!([sha.clone()]);
        admitted_sha["invalid"] = json!(["v"]);
        if let Some(errors) = admitted_sha["errors"].as_object_mut() {
            errors.shift_remove(&sha);
        }
        assert!(
            judge(
                &session,
                &answer(admitted_v, &["v"]),
                &answer(admitted_sha, &[&sha])
            )
            .is_failure()
        );
    }

    /// A step's answer whose contents after it hold `all`, each transaction of a sender of its own
    /// but those `senders` list together (oldest first).
    fn readd_step(result: Value, all: &[&str], senders: &[&[&str]]) -> Value {
        let mut by_sender: Vec<Value> = senders
            .iter()
            .enumerate()
            .map(|(at, list)| json!([format!("sender {at}"), list]))
            .collect();
        for id in all {
            if !senders.iter().any(|list| list.contains(id)) {
                by_sender.push(json!([format!("sender of {id}"), [id]]));
            }
        }
        let lowest: Vec<&str> = all.iter().rev().copied().collect();
        json!({"result": result, "contents": {"all": all, "bySender": by_sender, "size": all.len(),
                                             "highest": all, "lowest": lowest}})
    }

    #[test]
    fn an_eviction_at_a_re_add_is_documented_when_the_attempts_before_it_agree() {
        // The reference's re-add put back e and f, then tried r in its full pool: it evicted e,
        // then refused r; Heartwood's re-add refused r without evicting, so its pool keeps e. The
        // attempt after it (g) went its own way on each side.
        let notes = json!({"fullPoolEviction": true, "evictions": [{"id": "r", "evicted": ["e"]}],
                           "attempts": [["e", true], ["f", true], ["r", false, true], ["g", true]]});
        let theirs = run(vec![readd_step(
            json!({ "notes": notes }),
            &["f", "g"],
            &[],
        )]);
        let ours = |attempts: Value| {
            run(vec![readd_step(
                json!({"notes": {READD_ATTEMPTS: attempts}}),
                &["e", "f"],
                &[],
            )])
        };
        let eviction = Outcome::Documented(Documented::FullPoolEviction.reason().into());
        let agreed = json!([["e", true], ["f", true], ["r", false], ["g", false]]);
        for op in [
            json!({"op": "readd", "recheck": false, "forged": []}),
            json!({"op": "restart"}),
        ] {
            let session = json!({ "steps": [op] });
            assert_eq!(judge(&session, &theirs, &ours(agreed.clone())), eviction);
            // An attempt before the eviction with another outcome, another transaction or
            // another order: a mismatch.
            for differing in [
                json!([["e", true], ["f", false], ["r", false]]),
                json!([["e", true], ["x", true], ["r", false]]),
                json!([["f", true], ["e", true], ["r", false]]),
            ] {
                assert!(judge(&session, &theirs, &ours(differing)).is_failure());
            }
            // Heartwood admitted the noted transaction, or stopped before it, or noted nothing.
            assert!(
                judge(
                    &session,
                    &theirs,
                    &ours(json!([["e", true], ["f", true], ["r", true]]))
                )
                .is_failure()
            );
            assert!(
                judge(&session, &theirs, &ours(json!([["e", true], ["f", true]]))).is_failure()
            );
            assert!(
                judge(
                    &session,
                    &theirs,
                    &run(vec![readd_step(json!({}), &["e", "f"], &[])])
                )
                .is_failure()
            );
            // Pools that agree all the same are compared as in any other step.
            let same = run(vec![readd_step(
                json!({"notes": {READD_ATTEMPTS: agreed}}),
                &["f", "g"],
                &[],
            )]);
            assert_eq!(judge(&session, &theirs, &same), Outcome::Pass);
        }
        // Without the oracle's note, pools that differ after a re-add are a mismatch.
        let plain = run(vec![readd_step(json!({}), &["f", "g"], &[])]);
        let session = json!({"steps": [{"op": "readd"}]});
        assert!(judge(&session, &plain, &ours(agreed)).is_failure());
    }

    #[test]
    fn a_re_add_eviction_leaves_only_the_evicted_transaction_and_its_senders_later_ones_apart() {
        // The re-add of s's stored copy, a duplicate, made the reference's full pool evict l and
        // then refuse s; Heartwood refused s without evicting, so its pool keeps l, and l2, the
        // next transaction of l's sender, which the reference refused for its nonce.
        let notes = |refused: &str| {
            json!({"notes": {"fullPoolEviction": true,
                             "evictions": [{"id": refused, "evicted": ["l"]}],
                             "attempts": [["s", true], ["l", true], [refused, false, true],
                                          ["l2", false]]}})
        };
        let ours = |entered: bool| {
            json!({"notes": {READD_ATTEMPTS: [["s", true], ["l", true], ["s", false],
                                               ["l2", entered]]}})
        };
        let documented = Outcome::Documented(Documented::FullPoolEviction.reason().into());
        let theirs = run(vec![readd_step(notes("s"), &["s"], &[])]);
        for op in ["readd", "restart"] {
            let session = json!({"steps": [{"op": op}]});
            let kept = run(vec![readd_step(ours(false), &["s", "l"], &[])]);
            assert_eq!(judge(&session, &theirs, &kept), documented);
        }
        let session = json!({"steps": [{"op": "readd"}]});
        let next = run(vec![readd_step(
            ours(true),
            &["s", "l", "l2"],
            &[&["l", "l2"]],
        )]);
        assert_eq!(judge(&session, &theirs, &next), documented);
        // A transaction only Heartwood holds that the reference neither evicted nor refused for
        // an evicted predecessor: a mismatch.
        let other = run(vec![readd_step(ours(false), &["s", "l", "x"], &[])]);
        assert!(judge(&session, &theirs, &other).is_failure());
        // l2 of a sender of its own is no later transaction of the evicted one's sender.
        let apart = run(vec![readd_step(ours(true), &["s", "l", "l2"], &[])]);
        assert!(judge(&session, &theirs, &apart).is_failure());
        // Heartwood holds n, the transaction the reference refused after its eviction, which the
        // reference's pool does not hold.
        let refused = run(vec![readd_step(notes("n"), &["s"], &[])]);
        let ours_n = json!({"notes": {READD_ATTEMPTS: [["s", true], ["l", true], ["n", false],
                                                     ["l2", false]]}});
        let admitted = run(vec![readd_step(ours_n, &["s", "l", "n"], &[])]);
        assert!(judge(&session, &refused, &admitted).is_failure());
    }

    #[test]
    fn a_re_add_eviction_is_documented_only_where_the_rest_of_the_pool_agrees() {
        // A re-add tries h, e, f, r, g and q in a pool of three: the reference evicts e for r and
        // refuses r, admits g into the free place, then evicts g for q and refuses q; Heartwood
        // refuses r, g (its pool is still full, and g's fee is below e's) and q, and keeps e.
        let theirs_notes = json!({"notes": {"fullPoolEviction": true,
            "evictions": [{"id": "r", "evicted": ["e"]}, {"id": "q", "evicted": ["g"]}],
            "attempts": [["h", true], ["e", true], ["f", true], ["r", false, true], ["g", true],
                         ["q", false, true]]}});
        let ours_notes = json!({"notes": {READD_ATTEMPTS: [["h", true], ["e", true], ["f", true],
                                                            ["r", false], ["g", false], ["q", false]]}});
        let theirs = run(vec![json!({"result": theirs_notes, "contents": {
            "all": ["h", "f"], "highest": ["h", "f"], "lowest": ["f", "h"], "size": 2,
            "bySender": [["F", ["f"]], ["H", ["h"]]],
            "wallets": [["F", "100", "6"], ["H", "200", "3"]],
            "stored": [["f", 9], ["h", 9]]}})]);
        let ours = |contents: Value| run(vec![json!({"result": ours_notes, "contents": contents})]);
        let kept = json!({
            "all": ["h", "f", "e"], "highest": ["h", "f", "e"], "lowest": ["e", "f", "h"], "size": 3,
            "bySender": [["E", ["e"]], ["F", ["f"]], ["H", ["h"]]],
            "wallets": [["E", "50", "2"], ["F", "100", "6"], ["H", "200", "3"]],
            "stored": [["e", 9], ["f", 9], ["h", 9]]});
        let session = json!({"steps": [{"op": "readd", "forged": []}]});
        let documented = Outcome::Documented(Documented::FullPoolEviction.reason().into());
        // g, evicted by the reference, never entered Heartwood's pool: documented.
        assert_eq!(judge(&session, &theirs, &ours(kept.clone())), documented);
        // What the evictions do not touch must agree: the pool's size, the order of the
        // transactions both pools hold, another sender's transactions, wallet or stored rows.
        let changed = |pointer: &str, value: Value| {
            let mut contents = kept.clone();
            if let Some(slot) = contents.pointer_mut(pointer) {
                *slot = value;
            }
            ours(contents)
        };
        for differing in [
            changed("/size", json!(9)),
            changed("/highest", json!(["f", "h", "e"])),
            changed("/wallets/1", json!(["F", "100", "7"])),
            changed("/stored", json!([["e", 9], ["h", 9]])),
            changed("/bySender/1", json!(["F", ["f", "x"]])),
            changed("/lowest", json!(["e", "e", "h"])),
        ] {
            assert!(judge(&session, &theirs, &differing).is_failure());
        }
        // The evicted sender's own wallet and row differ on purpose.
        let own = changed("/wallets/0", json!(["E", "40", "3"]));
        assert_eq!(judge(&session, &theirs, &own), documented);
        // A transaction the re-add never tried, under the evicted one's sender: a mismatch.
        let mut invented = kept.clone();
        invented["all"] = json!(["h", "f", "e", "x"]);
        invented["highest"] = json!(["h", "f", "e", "x"]);
        invented["lowest"] = json!(["x", "e", "f", "h"]);
        invented["size"] = json!(4);
        invented["bySender"][0] = json!(["E", ["e", "x"]]);
        assert!(judge(&session, &theirs, &ours(invented)).is_failure());
    }

    #[test]
    fn a_sender_back_after_its_eviction_may_move_in_the_priority_order() {
        // A re-add of the forged e and f, with the rows of r and e stored (and f's from its
        // insert): the reference evicts e for r, refuses r, then admits e's stored row into its
        // free place, its sender's pool made again after f's; Heartwood refuses r, and the stored
        // e and f as duplicates. Both pools hold e and f, in another order where their fees tie.
        let theirs_notes = json!({"notes": {"fullPoolEviction": true,
            "evictions": [{"id": "r", "evicted": ["e"]}],
            "attempts": [["e", true], ["f", true], ["r", false, true], ["e", true],
                         ["f", false]]}});
        let ours_notes = json!({"notes": {READD_ATTEMPTS: [["e", true], ["f", true], ["r", false],
                                                            ["e", false], ["f", false]]}});
        let theirs = run(vec![json!({"result": theirs_notes, "contents": {
            "all": ["f", "e"], "highest": ["f", "e"], "lowest": ["e", "f"], "size": 2,
            "bySender": [["F", ["f"]], ["E", ["e"]]],
            "wallets": [["F", "100", "6"], ["E", "50", "2"]],
            "stored": [["e", 9], ["f", 9]]}})]);
        let ours = run(vec![json!({"result": ours_notes, "contents": {
            "all": ["e", "f"], "highest": ["e", "f"], "lowest": ["f", "e"], "size": 2,
            "bySender": [["E", ["e"]], ["F", ["f"]]],
            "wallets": [["E", "50", "2"], ["F", "100", "6"]],
            "stored": [["f", 9]]}})]);
        let session = json!({"steps": [{"op": "readd", "forged": ["e", "f"]}]});
        assert_eq!(
            judge(&session, &theirs, &ours),
            Outcome::Documented(Documented::FullPoolEviction.reason().into())
        );
    }

    #[test]
    fn a_re_add_eviction_keeps_what_both_pools_held_before_it() {
        // A restart re-adds f and l, then tries s in its full pool: the reference evicts l and
        // refuses s, then has room for t and u; Heartwood refuses s without evicting, keeps l and
        // has no room for t or u.
        let session = json!({"steps": [{"op": "restart"}]});
        let theirs_notes = json!({"notes": {"fullPoolEviction": true,
                                           "evictions": [{"id": "s", "evicted": ["l"]}],
                                           "attempts": [["f", true], ["l", true], ["s", false, true],
                                                        ["t", true], ["u", true]]}});
        let answer = |result: &Value, all: &[&str]| run(vec![readd_step(result.clone(), all, &[])]);
        let ours = |t: bool, u: bool| {
            json!({"notes": {READD_ATTEMPTS: [["f", true], ["l", true], ["s", false], ["t", t],
                                               ["u", u]]}})
        };
        let documented = Outcome::Documented(Documented::FullPoolEviction.reason().into());
        let expected = answer(&theirs_notes, &["f", "t", "u"]);
        assert_eq!(
            judge(
                &session,
                &expected,
                &answer(&ours(false, false), &["f", "l"])
            ),
            documented
        );
        // f, tried before the eviction, is missing from Heartwood's pool: a mismatch.
        assert!(judge(&session, &expected, &answer(&ours(false, false), &["l"])).is_failure());
        // l, which the reference evicted, is missing from Heartwood's pool with nothing tried
        // after the refusal in its place: a mismatch.
        assert!(judge(&session, &expected, &answer(&ours(false, false), &["f"])).is_failure());
        // Heartwood evicted l only to admit u, tried after the refusal, and had no room for t.
        assert_eq!(
            judge(
                &session,
                &expected,
                &answer(&ours(false, true), &["f", "u"])
            ),
            documented
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
