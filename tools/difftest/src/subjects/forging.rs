//! Block assembly and signing, answered with `heartwood_forger::delegate`:
//!
//! - `forger.forge` `{privateKey, aux, previousBlock: {id, height}, timestamp, reward,
//!   transactions, milestones?}` -> `{hex, id}`: each transaction read as `fromBytesUnsafe` reads
//!   it, under the rules of the previous block's height (the forger reads the relay's answer at
//!   the network state's height), then `Delegate.forge`;
//! - `forger.collateAndForge`: `pool.run`'s input with `{privateKey, aux, previousBlock: {id},
//!   timestamp, reward}` -> `{collated, removed, hex, id}`: the pool session's steps, a forger's
//!   collation (`validate` on, nothing excluded), the ids the collation removed from the pool
//!   (sorted), and `Delegate.forge` on the collated transactions after the pool's tip (see
//!   `tools/oracle/lib/ops/forger-forge.js`). Both answers carry the session in their notes, as
//!   `pool.run` answers it with the collation as its last step (`session`); Heartwood's also
//!   carries the pool's tip (`tip`) and the bytes of each transaction the steps name, by id
//!   (`transactions`).
//!
//! The pool session runs through this crate's `pool.run` subject, with the forger's collation as
//! its last step; the collated transactions are found among the steps' entries by their ids.
//! Answers that differ are judged by [`collation_difference`]: where the two sessions show one of
//! the pool's documented differences that leave the pools holding different transactions, the case
//! is that difference, provided the forging answers differ only where the pools do.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, KeyPair, Milestones, SecretKey, Transaction};
use heartwood_forger::{Delegate, ForgeInputs};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, amount, bytes32, decode_refused, failed, integer, text};
use crate::compare::{self, Difference};
use crate::report::Outcome as Judged;
use crate::subject::{Case, Documented, NOTES, Registry, Verdict};

/// The operations of block assembly.
pub(super) const OPS: &[(&str, Run)] = &[
    ("forger.forge", forge),
    ("forger.collateAndForge", collate_and_forge),
];

/// The keys of `forger.collateAndForge`'s input that are the forger's, not the pool session's.
const FORGER_KEYS: [&str; 5] = ["privateKey", "aux", "previousBlock", "timestamp", "reward"];

/// The operation whose answers [`collation_difference`] judges.
pub const COLLATE_AND_FORGE: &str = "forger.collateAndForge";

/// The note that holds a side's pool session, with the collation as its last step.
const SESSION: &str = "session";

/// The note of Heartwood's answer that holds the bytes of each transaction the steps name, by id.
const TRANSACTIONS: &str = "transactions";

/// The note of Heartwood's answer that holds the pool's tip, the height the block follows.
const TIP: &str = "tip";

/// The note of Heartwood's answer that holds the block's limits at the tip, which the collation
/// keeps: `maxTransactions` and `maxPayload` (null where the milestone sets none).
const LIMITS: &str = "limits";

/// The bytes of a block header before its transactions, which the collation's byte budget leaves
/// out.
const BLOCK_HEADER_SIZE: i64 = 141;

/// The pool's documented differences after which the two pools hold different transactions, so
/// that the forger collates different ones on purpose.
const POOL_DIFFERENCES: [Documented; 2] = [Documented::FullPoolEviction, Documented::OnlineView];

/// The step a forger's collation adds to a pool session: `validate` on, nothing excluded.
fn collation_step() -> Value {
    json!({"op": "collate", "validate": true, "exclude": []})
}

fn forge(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let milestones = env.milestones(case)?;
    let input = case.input;
    let previous: u32 = integer(&input["previousBlock"], "height")?;
    let params = milestones.at(previous);
    let mut transactions = Vec::new();
    for item in input["transactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let bytes = item
            .as_str()
            .and_then(|text| hex::decode(text).ok())
            .ok_or_else(|| failed("a transaction is not hex"))?;
        match Transaction::decode_trusted(&bytes, params) {
            Ok(transaction) => transactions.push(transaction),
            Err(error) => return Ok(decode_refused(&error, &bytes)),
        }
    }
    forged(env, input, &transactions, previous, &milestones)
}

fn collate_and_forge(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let session = input
        .as_object()
        .ok_or_else(|| failed("the input is not an object"))?;
    let mut pool_input: Map<String, Value> = (session.iter())
        .filter(|(key, _)| !FORGER_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut steps = input["steps"].as_array().cloned().unwrap_or_default();
    let user_steps = steps.len();
    steps.push(collation_step());
    pool_input.insert("steps".into(), Value::Array(steps.clone()));
    let pool_input = Value::Object(pool_input);
    let pool_case = Case {
        op: "pool.run",
        input: &pool_input,
        ..*case
    };
    let run = (super::pool::OPS.iter())
        .find(|(op, _)| *op == "pool.run")
        .map(|(_, run)| *run)
        .ok_or_else(|| failed("no pool.run subject"))?;
    let Verdict::Output(session) = run(env, &pool_case)? else {
        return Err(failed("the pool session was refused"));
    };
    let results = session["results"]
        .as_array()
        .ok_or_else(|| failed("the pool session has no results"))?;
    let ids = |result: Option<&Value>| -> BTreeSet<String> {
        (result.and_then(|r| r["contents"]["all"].as_array()))
            .map(|all| all.iter().filter_map(Value::as_str).map(str::to_owned))
            .into_iter()
            .flatten()
            .collect()
    };
    let before = ids(user_steps.checked_sub(1).and_then(|i| results.get(i)));
    let after = ids(results.get(user_steps));
    let removed: Vec<&String> = before.difference(&after).collect();
    let collated: Vec<String> = (results.get(user_steps))
        .and_then(|r| r["result"]["ids"].as_array())
        .ok_or_else(|| failed("the collation has no ids"))?
        .iter()
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();

    // The pool's tip after the steps, and the collated transactions as the forger reads them.
    let milestones = env.milestones(case)?;
    let last: u32 = integer(input, "lastBlockHeight")?;
    let tip = (steps.iter().rev())
        .find(|step| step["op"] == "tip")
        .map_or(Ok(last), |step| integer(step, "height"))?;
    let entries = entries(&steps, &milestones, tip);
    let mut transactions = Vec::new();
    for id in &collated {
        let bytes = entries
            .get(id)
            .ok_or_else(|| failed(format!("the collated {id} is not among the entries")))?;
        match Transaction::decode_trusted(bytes, milestones.at(tip)) {
            Ok(transaction) => transactions.push(transaction),
            Err(error) => return Ok(decode_refused(&error, bytes)),
        }
    }
    let previous = json!({"id": input["previousBlock"]["id"], "height": tip});
    let mut forger = input.clone();
    forger["previousBlock"] = previous;
    let block = forged(env, &forger, &transactions, tip, &milestones)?;
    let named: Map<String, Value> = (entries.iter())
        .map(|(id, bytes)| (id.clone(), Value::from(hex::encode(bytes))))
        .collect();
    let rules = milestones.at(tip);
    let limits = json!({
        "maxTransactions": rules.max_transactions(), "maxPayload": rules.max_payload(),
    });
    Ok(match block {
        Verdict::Output(block) => Verdict::Output(json!({
            "collated": collated, "removed": removed, "hex": block["hex"], "id": block["id"],
            NOTES: {SESSION: session, TIP: tip, TRANSACTIONS: named, LIMITS: limits},
        })),
        refused => refused,
    })
}

/// The outcome of a `forger.collateAndForge` case whose answers, the oracle's `expected` and
/// Heartwood's `actual`, differ by `differences`.
///
/// The case is one of the pool's documented differences only where the sessions in the two
/// answers' notes show one after which the pools hold different transactions (a full pool's
/// eviction, in a batch or a re-add, or the online record in a sender's view: `POOL_DIFFERENCES`),
/// judged by the pool's own judge ([`crate::levels::pool::judge`]), and where the forging answers
/// differ only as the pools do (`unexplained_collation`):
///
/// - each side's collation follows from its own pool as the collator chooses: in priority order,
///   each transaction collated or removed, until the block is full;
/// - the collators decide alike (collated, or removed) each transaction that both pools held and
///   both reached before a limit stopped them, and collate those they both collated in the same
///   order, apart from the transactions of the senders the difference concerns
///   (`moved_by_difference`), whose place among equal fees may differ; a transaction one of them
///   did not reach is decided by the other alone;
/// - Heartwood's block holds its collation and agrees with the reference's block in every field
///   that does not depend on the transactions, and in its verification (`block_unexplained`);
/// - Heartwood's forger makes the reference's block, byte for byte, from the reference's collated
///   transactions, so the blocks differ only by the transactions in them.
///
/// Otherwise the case fails: with the session's own differences when the pool's judge fails it,
/// and with the forging answers' differences (and the check that failed) when the sessions agree
/// or a check fails.
pub fn collation_difference(
    case: &Case<'_>,
    expected: &Value,
    actual: &Value,
    registry: &Registry,
    differences: Vec<Difference>,
) -> Judged {
    let (Some(theirs), Some(ours)) = (expected[NOTES].get(SESSION), actual[NOTES].get(SESSION))
    else {
        return Judged::Mismatch(differences);
    };
    let mut steps = case.input["steps"].as_array().cloned().unwrap_or_default();
    let user_steps = steps.len();
    steps.push(collation_step());
    let difference = match crate::levels::pool::judge(&json!({ "steps": steps }), theirs, ours) {
        Judged::Documented(reason) => {
            match POOL_DIFFERENCES.into_iter().find(|d| d.reason() == reason) {
                Some(difference) => difference,
                // A difference that changes no pool's contents: the pools agree.
                None => return Judged::Mismatch(differences),
            }
        }
        Judged::Pass => return Judged::Mismatch(differences),
        failed => return failed,
    };
    let unexplained = |mut found: Vec<Difference>| {
        let mut all = differences.clone();
        all.append(&mut found);
        Judged::Mismatch(all)
    };
    // The pools before the collation: the contents after the last step of the session's own.
    let pool = |session: &Value| {
        (user_steps.checked_sub(1))
            .and_then(|last| session["results"].get(last))
            .map_or(Value::Null, |step| step["contents"].clone())
    };
    let moved = moved_by_difference(theirs, ours, user_steps);
    if let Some(found) = unexplained_collation(&pool(theirs), expected, &pool(ours), actual, &moved)
    {
        return unexplained(vec![found]);
    }
    if let Some(found) = block_unexplained(case, expected, actual, registry) {
        return unexplained(vec![found]);
    }
    let theirs_block = json!({"hex": expected["hex"], "id": expected["id"]});
    match reference_block(case, expected, actual, registry) {
        Ok(block) => {
            let found = compare::diff(
                &theirs_block,
                &json!({"hex": block["hex"], "id": block["id"]}),
            );
            if !found.is_empty() {
                return unexplained(
                    (found.into_iter())
                        .map(|d| rebuilt_path(d, "the reference's collation forged by Heartwood"))
                        .collect(),
                );
            }
        }
        Err(problem) => {
            return unexplained(vec![Difference::Value {
                path: "the reference's collation forged by Heartwood".into(),
                expected: theirs_block,
                actual: Value::from(problem),
            }]);
        }
    }
    Judged::Documented(difference.reason().to_owned())
}

/// The ids of `list`, a JSON array of ids.
fn ids(list: &Value) -> Vec<String> {
    (list.as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Where the two collations, the reference's (`theirs`, of its pool before the collation
/// `their_pool`) and Heartwood's (`ours`, of `our_pool`), differ otherwise than the two pools do;
/// `None` when they differ only as the pools do. A pool is the contents after the session's last
/// step (`all`, `highest`), an answer gives `collated` and `removed`, and Heartwood's notes give
/// the bytes of each transaction and the block's limits. A transaction both pools held is
/// compared where both collators reached it: there it is collated on both sides or removed on
/// both, and the ones both collated stand in the same order, leaving out the transactions of
/// `moved`, whose senders the documented difference concerns (see [`moved_by_difference`]).
/// Where a limit stopped one collator before it, the other's decision stands alone (the pools'
/// other transactions moved the limit).
fn unexplained_collation(
    their_pool: &Value,
    theirs: &Value,
    our_pool: &Value,
    ours: &Value,
    moved: &BTreeSet<String>,
) -> Option<Difference> {
    let problem = |path: &str, what: String| Difference::Value {
        path: path.to_owned(),
        expected: Value::from("a collation that follows from the pool"),
        actual: Value::from(what),
    };
    let notes = &ours[NOTES];
    // Each side's collation follows from its own pool, so a transaction of its pool that it
    // neither collated nor removed lies beyond the limit that stopped it: it was never tried.
    if let Err(why) = follows(their_pool, theirs, notes) {
        return Some(problem("$.collated, the reference's", why));
    }
    if let Err(why) = follows(our_pool, ours, notes) {
        return Some(problem("$.collated, Heartwood's", why));
    }
    let in_ours: BTreeSet<String> = ids(&our_pool["all"]).into_iter().collect();
    let decisions = |answer: &Value| -> BTreeMap<String, &'static str> {
        let mut out: BTreeMap<String, &'static str> = (ids(&answer["removed"]).into_iter())
            .map(|id| (id, "removed"))
            .collect();
        out.extend(
            ids(&answer["collated"])
                .into_iter()
                .map(|id| (id, "collated")),
        );
        out
    };
    let (their_decisions, our_decisions) = (decisions(theirs), decisions(ours));
    // The transactions both pools held that both collators reached: decided alike.
    for id in ids(&their_pool["all"])
        .iter()
        .filter(|id| in_ours.contains(*id))
    {
        if let (Some(&a), Some(&b)) = (their_decisions.get(id), our_decisions.get(id))
            && a != b
        {
            return Some(Difference::Value {
                path: format!(
                    "$.collated and $.removed: {id}, which both pools held and both collators \
                     reached"
                ),
                expected: json!(a),
                actual: json!(b),
            });
        }
    }
    // And those both collated, in the same order, but for the senders the difference concerns.
    let both = |answer: &Value, other: &BTreeMap<String, &'static str>| -> Vec<String> {
        (ids(&answer["collated"]).into_iter())
            .filter(|id| other.get(id) == Some(&"collated") && !moved.contains(id))
            .collect()
    };
    let (a, b) = (both(theirs, &our_decisions), both(ours, &their_decisions));
    if a != b {
        return Some(Difference::Value {
            path: "$.collated, of the transactions both pools held and both collated, of the \
                   senders the pools' difference does not concern"
                .into(),
            expected: json!(a),
            actual: json!(b),
        });
    }
    None
}

/// The transactions whose place among equal fees the pools' documented difference may change: in
/// either session (`theirs`, the reference's, and `ours`), each transaction any step's contents
/// list for a sender the difference concerns. A sender is concerned when, in the session's own
/// `steps` (the collation after them left out), the reference's full pool evicted one of its
/// transactions (the evictions the oracle notes in a step's result, in a batch or a re-add), or
/// one pool held one of its transactions after a step and the other did not.
///
/// The reference orders transactions of equal fees by the order in which their senders' mempools
/// were made (`getFromHighestPriority` merges the senders' lists in the mempool's insertion order,
/// and a tie keeps the earlier sender), and a sender whose mempool emptied is made again last when
/// a transaction of its own is admitted. After an eviction that Heartwood does not make, the
/// evicted transaction admitted again (sent again, or re-added) comes after the senders of the same
/// fee in the reference's order, while Heartwood's pool kept it in its place. The pools' own
/// priority orders are checked against each collation by `follows`.
fn moved_by_difference(theirs: &Value, ours: &Value, steps: usize) -> BTreeSet<String> {
    let results = |session: &Value| -> Vec<Value> {
        session["results"].as_array().cloned().unwrap_or_default()
    };
    let (their_steps, our_steps) = (results(theirs), results(ours));
    // Each sender's transactions, and each transaction's sender, from every step of both sessions.
    let mut held_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut sender_of: BTreeMap<String, String> = BTreeMap::new();
    for step in their_steps.iter().chain(&our_steps) {
        for sender in step["contents"]["bySender"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(address) = sender[0].as_str() else {
                continue;
            };
            for id in ids(&sender[1]) {
                sender_of.insert(id.clone(), address.to_owned());
                held_by.entry(address.to_owned()).or_default().insert(id);
            }
        }
    }
    let evicted = (their_steps.iter().take(steps))
        .flat_map(|step| ids_of_evictions(&step["result"][NOTES]["evictions"]));
    let held =
        |step: &Value| -> BTreeSet<String> { ids(&step["contents"]["all"]).into_iter().collect() };
    let one_side = (their_steps.iter().zip(&our_steps).take(steps)).flat_map(|(a, b)| {
        held(a)
            .symmetric_difference(&held(b))
            .cloned()
            .collect::<Vec<_>>()
    });
    let concerned: BTreeSet<&String> = evicted
        .chain(one_side)
        .filter_map(|id| sender_of.get(&id))
        .collect();
    (concerned.into_iter())
        .filter_map(|address| held_by.get(address))
        .flatten()
        .cloned()
        .collect()
}

/// The evicted ids of the oracle's eviction notes (`[{id, evicted: [id]}]`).
fn ids_of_evictions(evictions: &Value) -> Vec<String> {
    (evictions.as_array().into_iter().flatten())
        .flat_map(|eviction| ids(&eviction["evicted"]))
        .collect()
}

/// Whether `answer`'s collation follows from `pool` as the collator chooses it. Down the pool's
/// priority order (`highest`, which must keep each sender's transactions in the order `bySender`
/// lists them, oldest first):
///
/// - a transaction whose sender's earlier one was removed is removed too (the removal takes the
///   sender's later transactions with it), wherever it stands;
/// - any other transaction is collated, in that order, while the block has room for it; or it is
///   removed (it failed when the collator tried it, which it did only with room in the block); or
///   it is neither, where the block is full, by `maxTransactions` or by the byte budget of
///   `notes` (`maxPayload` less the header, then 4 bytes and the transaction's own each), and no
///   later one is collated or fails.
///
/// `Ok(true)` when a limit stopped it, `Ok(false)` when it went through the whole pool; why not,
/// when it does not follow.
fn follows(pool: &Value, answer: &Value, notes: &Value) -> Result<bool, String> {
    let collated = ids(&answer["collated"]);
    let removed: BTreeSet<String> = ids(&answer["removed"]).into_iter().collect();
    // The pool's views hold the same transactions, each once.
    let held: BTreeSet<String> = ids(&pool["all"]).into_iter().collect();
    let highest = ids(&pool["highest"]);
    let senders: Vec<String> = (pool["bySender"].as_array().into_iter().flatten())
        .flat_map(|sender| ids(&sender[1]))
        .collect();
    let same = |list: &[String]| {
        list.len() == held.len() && list.iter().collect::<BTreeSet<_>>() == held.iter().collect()
    };
    if !same(&highest) || !same(&senders) {
        return Err(
            "the pool's priority order and its senders' lists do not hold the same \
                    transactions as the pool"
                .into(),
        );
    }
    if let Some(id) = removed.iter().find(|id| !held.contains(*id)) {
        return Err(format!("{id} is removed but was not in the pool"));
    }
    // Each transaction's sender's earlier ones (`bySender` lists them oldest first), which the
    // priority order keeps before it: the collator takes a sender's transactions in nonce order.
    let place: BTreeMap<&str, usize> = (highest.iter().enumerate())
        .map(|(at, id)| (id.as_str(), at))
        .collect();
    let mut earlier: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for sender in pool["bySender"].as_array().into_iter().flatten() {
        let list = ids(&sender[1]);
        let places: Vec<Option<&usize>> = list.iter().map(|id| place.get(id.as_str())).collect();
        if let Some(pair) = list
            .windows(2)
            .zip(places.windows(2))
            .find(|(_, at)| at[0] >= at[1])
        {
            return Err(format!(
                "{} comes before {}, its sender's earlier transaction, in the priority order",
                pair.0[1], pair.0[0]
            ));
        }
        for (at, id) in list.iter().enumerate() {
            earlier.insert(id.clone(), list[..at].to_vec());
        }
    }
    let size = |id: &str| -> Result<i64, String> {
        (notes[TRANSACTIONS][id].as_str())
            .map(|text| i64::try_from(text.len() / 2).unwrap_or(i64::MAX))
            .ok_or_else(|| format!("{id} is named by no step"))
    };
    let max = notes[LIMITS]["maxTransactions"].as_u64();
    let mut left = (notes[LIMITS]["maxPayload"].as_i64())
        .map(|payload| payload.saturating_sub(BLOCK_HEADER_SIZE));
    // Whether the block has no room for `id` after `count` transactions, with `left` bytes.
    let full = |count: usize, left: Option<i64>, id: &str| -> Result<bool, String> {
        let by_count = max.is_some_and(|max| u64::try_from(count).is_ok_and(|n| n >= max));
        let by_bytes = match left {
            Some(left) => left.saturating_sub(4).saturating_sub(size(id)?) < 0,
            None => false,
        };
        Ok(by_count || by_bytes)
    };
    let mut next = 0;
    let mut stop: Option<String> = None;
    for id in highest {
        let after_removal =
            (earlier.get(&id)).is_some_and(|before| before.iter().any(|e| removed.contains(e)));
        if after_removal {
            if !removed.contains(&id) {
                return Err(format!(
                    "{id} is kept after its sender's earlier transaction was removed"
                ));
            }
            continue;
        }
        if removed.contains(&id) {
            if stop.is_some() || full(next, left, &id)? {
                return Err(format!(
                    "{id} is removed where the block had no room left to try it"
                ));
            }
            continue;
        }
        if let Some(stop) = &stop {
            if collated.contains(&id) {
                return Err(format!(
                    "{id} is collated after {stop}, where the block was full"
                ));
            }
            continue;
        }
        let no_room = full(next, left, &id)?;
        if collated.get(next) == Some(&id) {
            if no_room {
                return Err(format!("{id} is collated beyond the block's limits"));
            }
            if let Some(bytes) = &mut left {
                *bytes = bytes.saturating_sub(4).saturating_sub(size(&id)?);
            }
            next += 1;
            continue;
        }
        if !no_room {
            return Err(format!(
                "{id} is neither collated nor removed, and the block has room for it"
            ));
        }
        stop = Some(id);
    }
    if next != collated.len() {
        return Err(format!(
            "{} is collated out of the pool's priority order, or is not in the pool",
            collated.get(next).map_or("a transaction", String::as_str)
        ));
    }
    Ok(stop.is_some())
}

/// The fields of a block that do not depend on its transactions, which two blocks forged with
/// the same inputs share whatever they hold.
const SHARED_FIELDS: [&str; 7] = [
    "version",
    "timestamp",
    "height",
    "previousBlock",
    "reward",
    "generatorPublicKey",
    "donations",
];

/// Where Heartwood's block (`actual`) is not its collation in a block forged as the reference's
/// (`expected`) was; `None` when it is. Both blocks are read with the `block.fromBytes` subject:
/// Heartwood's id must be that of its bytes, its bytes the block they decode to written again (no
/// trailing bytes), its transactions its collation in order, its fields that do not depend on the
/// transactions the reference block's, and its verification the reference block's (signature,
/// payload hash and totals included), by the rules that fail, whatever their numbers.
fn block_unexplained(
    case: &Case<'_>,
    expected: &Value,
    actual: &Value,
    registry: &Registry,
) -> Option<Difference> {
    let read = |hex: &Value| -> Result<Value, String> {
        let mut input = json!({ "hex": hex });
        if let Some(milestones) = case.input.get("milestones") {
            input["milestones"] = milestones.clone();
        }
        let request = Case {
            op: "block.fromBytes",
            input: &input,
            ..*case
        };
        let subject =
            (registry.get(request.op)).ok_or("no block.fromBytes subject is registered")?;
        match subject.run(&request) {
            Ok(Verdict::Output(block)) => Ok(block),
            Ok(other) => Err(format!("the block is refused when read: {other:?}")),
            Err(error) => Err(error.to_string()),
        }
    };
    let problem = |what: String| Difference::Value {
        path: "$.hex, Heartwood's block".into(),
        expected: Value::from("Heartwood's collation, forged as the reference's block was"),
        actual: Value::from(what),
    };
    let (theirs, ours) = match (read(&expected["hex"]), read(&actual["hex"])) {
        (Ok(theirs), Ok(ours)) => (theirs, ours),
        (Err(why), _) => return Some(problem(format!("the reference's block: {why}"))),
        (_, Err(why)) => return Some(problem(why)),
    };
    if ours["id"] != actual["id"] {
        return Some(problem(format!(
            "its id {} is not {}, the id of its bytes",
            actual["id"], ours["id"]
        )));
    }
    if ours["hex"] != actual["hex"] {
        return Some(problem(
            "its bytes are not the block they decode to, written again".into(),
        ));
    }
    let transactions: Vec<Value> = (ours["json"]["transactions"]
        .as_array()
        .into_iter()
        .flatten())
    .map(|transaction| transaction["id"].clone())
    .collect();
    if Value::Array(transactions) != actual["collated"] {
        return Some(problem(
            "its transactions are not Heartwood's collation".into(),
        ));
    }
    if let Some(field) = SHARED_FIELDS
        .into_iter()
        .find(|field| ours["json"][*field] != theirs["json"][*field])
    {
        return Some(problem(format!("its {field} is not the reference block's")));
    }
    if verdict(&ours["verification"]) != verdict(&theirs["verification"]) {
        return Some(problem(format!(
            "its verification {} is not the reference block's {}",
            ours["verification"], theirs["verification"]
        )));
    }
    None
}

/// A block's verification as two blocks of different transactions share it: whether it is
/// verified, and each failed rule's text with its numbers left out (two blocks can both be over
/// the payload limit by different sizes), without the transactions a failure lists.
fn verdict(verification: &Value) -> (Value, Vec<String>) {
    let rules = (verification["errors"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .filter(|text| !text.starts_with("=> "))
        .map(|text| {
            let mut out = String::new();
            for c in text.chars() {
                if !c.is_ascii_digit() {
                    out.push(c);
                } else if !out.ends_with('#') {
                    out.push('#');
                }
            }
            out
        })
        .collect();
    (verification["verified"].clone(), rules)
}

/// Heartwood's block for the reference's collated transactions (`forger.forge` with the case's
/// key, aux, previous block, timestamp, reward and milestones, after the tip of Heartwood's
/// session), or why there is none.
fn reference_block(
    case: &Case<'_>,
    expected: &Value,
    actual: &Value,
    registry: &Registry,
) -> Result<Value, String> {
    let named = &actual[NOTES][TRANSACTIONS];
    let transactions = (expected["collated"].as_array().into_iter().flatten())
        .map(|id| {
            (id.as_str().and_then(|id| named.get(id)).cloned())
                .ok_or_else(|| format!("the reference collated {id}, which no step names"))
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let input = case.input;
    let mut forge = json!({
        "privateKey": input["privateKey"],
        "aux": input["aux"],
        "previousBlock": {"id": input["previousBlock"]["id"], "height": actual[NOTES][TIP]},
        "timestamp": input["timestamp"],
        "reward": input["reward"],
        "transactions": transactions,
    });
    if let Some(milestones) = input.get("milestones") {
        forge["milestones"] = milestones.clone();
    }
    let request = Case {
        op: "forger.forge",
        input: &forge,
        ..*case
    };
    let subject = registry
        .get(request.op)
        .ok_or("no forger.forge subject is registered")?;
    match subject.run(&request) {
        Ok(Verdict::Output(block)) => Ok(block),
        Ok(other) => Err(format!("Heartwood's forger refused it: {other:?}")),
        Err(error) => Err(error.to_string()),
    }
}

/// `difference` with `what` before its path.
fn rebuilt_path(difference: Difference, what: &str) -> Difference {
    let path = |path: String| format!("{what}: {path}");
    match difference {
        Difference::Value {
            path: at,
            expected,
            actual,
        } => Difference::Value {
            path: path(at),
            expected,
            actual,
        },
        Difference::Bytes {
            path: at,
            offset,
            expected_len,
            actual_len,
            expected_window,
            actual_window,
        } => Difference::Bytes {
            path: path(at),
            offset,
            expected_len,
            actual_len,
            expected_window,
            actual_window,
        },
        Difference::Missing { path: at, expected } => Difference::Missing {
            path: path(at),
            expected,
        },
        Difference::Extra { path: at, actual } => Difference::Extra {
            path: path(at),
            actual,
        },
        Difference::Order {
            path: at,
            expected,
            actual,
        } => Difference::Order {
            path: path(at),
            expected,
            actual,
        },
    }
}

/// The bytes of every transaction the steps name (the processed entries, the re-added and removed
/// ones), by the id the pool gives it under the rules after `tip`.
fn entries(steps: &[Value], milestones: &Milestones, tip: u32) -> BTreeMap<String, Vec<u8>> {
    let params = milestones.at(tip.saturating_add(1));
    let mut found = BTreeMap::new();
    let mut add = |transaction: Option<Transaction>| {
        if let Some(transaction) = transaction {
            found.insert(transaction.id().to_hex(), transaction.bytes().to_vec());
        }
    };
    let from_hex = |text: &Value| {
        let bytes = hex::decode(text.as_str()?).ok()?;
        (Transaction::decode(&bytes, params).ok())
            .or_else(|| Transaction::decode_trusted(&bytes, params).ok())
    };
    for step in steps {
        for entry in step["entries"].as_array().into_iter().flatten() {
            add(from_hex(&entry["hex"]));
            add(Transaction::from_json(&entry["json"], params).ok());
        }
        for text in step["forged"].as_array().into_iter().flatten() {
            add(from_hex(text));
        }
        add(from_hex(&step["hex"]));
    }
    found
}

/// `Delegate.forge` with the key, aux, previous block, timestamp and reward of `input`.
fn forged(
    env: &Env<'_>,
    input: &Value,
    transactions: &[Transaction],
    previous_height: u32,
    milestones: &Milestones,
) -> Outcome<Verdict> {
    let network = env.devnet()?.network.pub_key_hash;
    let secret = SecretKey::from_hex(text(input, "privateKey")?)
        .map_err(|e| failed(format!("input.privateKey: {e}")))?;
    let delegate = Delegate::new(Arc::new(KeyPair::from_secret_key(secret)), network);
    let previous_id = BlockId::from_array(bytes32(&input["previousBlock"], "id")?);
    let inputs = ForgeInputs {
        transactions,
        previous_id,
        previous_height,
        timestamp: integer(input, "timestamp")?,
        reward: amount(input, "reward")?,
    };
    let aux = Aux::fixed(bytes32(input, "aux")?);
    Ok(match delegate.forge(&inputs, aux, milestones) {
        Ok(block) => Verdict::Output(json!({
            "hex": hex::encode(&block.encode()),
            "id": block.id().to_hex(),
        })),
        // A refused block has no error class to compare: only the verdict.
        Err(error) => Verdict::Rejected {
            class: None,
            detail: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool before the collation, with its transactions from the highest priority down, each
    /// sender's (`senders`, oldest first) or each of a sender of its own.
    fn pool_of(highest: &[&str], senders: &[&[&str]]) -> Value {
        let mut by_sender: Vec<Value> = senders
            .iter()
            .enumerate()
            .map(|(at, list)| json!([format!("sender {at}"), list]))
            .collect();
        for id in highest {
            if !senders.iter().any(|list| list.contains(id)) {
                by_sender.push(json!([format!("sender of {id}"), [id]]));
            }
        }
        json!({"all": highest, "highest": highest, "bySender": by_sender})
    }

    fn pool(highest: &[&str]) -> Value {
        pool_of(highest, &[])
    }

    /// An answer: its collation and removals, with notes of 100-byte transactions named a to z,
    /// one of 1,000 bytes named `big`, and the block's limits.
    fn answer(collated: &[&str], removed: &[&str], max: u64, payload: u64) -> Value {
        let mut named: Map<String, Value> = (b'a'..=b'z')
            .map(|c| (char::from(c).to_string(), Value::from("00".repeat(100))))
            .collect();
        named.insert("big".into(), Value::from("00".repeat(1_000)));
        json!({"collated": collated, "removed": removed, NOTES: {TRANSACTIONS: named,
               LIMITS: {"maxTransactions": max, "maxPayload": payload}}})
    }

    /// Whether `answer` follows from `pool`, with its own notes.
    fn checked(pool: &Value, answer: &Value) -> Result<bool, String> {
        follows(pool, answer, &answer[NOTES])
    }

    /// Where the two collations differ otherwise than the pools do, with no sender the pools'
    /// difference concerns.
    fn unexplained(
        their_pool: &Value,
        theirs: &Value,
        our_pool: &Value,
        ours: &Value,
    ) -> Option<Difference> {
        unexplained_collation(their_pool, theirs, our_pool, ours, &BTreeSet::new())
    }

    #[test]
    fn a_collation_follows_its_pool_in_priority_order_until_the_block_is_full() {
        let free = |collated: &[&str], removed: &[&str]| answer(collated, removed, 150, 2_097_152);
        // Every transaction collated or removed, in priority order.
        assert_eq!(
            checked(&pool(&["s", "x", "t"]), &free(&["s", "t"], &["x"])),
            Ok(false)
        );
        // A sender's next transaction before its earlier one: out of the pool's order.
        assert!(checked(&pool(&["s", "l", "m"]), &free(&["s", "m", "l"], &[])).is_err());
        // A transaction left out with room in the block.
        assert!(checked(&pool(&["s", "x"]), &free(&["s"], &[])).is_err());
        // Left out at the count limit, or at the byte budget (141 + 2 x 104 bytes holds two).
        assert_eq!(
            checked(&pool(&["s", "x"]), &answer(&["s"], &[], 1, 2_097_152)),
            Ok(true)
        );
        let two = 141 + 2 * 104 + 103;
        assert_eq!(
            checked(&pool(&["s", "t", "x"]), &answer(&["s", "t"], &[], 150, two)),
            Ok(true)
        );
        // Collated beyond the count or the byte budget.
        assert!(checked(&pool(&["s", "x"]), &answer(&["s", "x"], &[], 1, 2_097_152)).is_err());
        let one = 141 + 104 + 103;
        assert!(checked(&pool(&["s", "x"]), &answer(&["s", "x"], &[], 150, one)).is_err());
        // Nothing is collated after the block was full.
        assert!(checked(&pool(&["s", "x", "y"]), &answer(&["s", "y"], &[], 2, one)).is_err());
    }

    #[test]
    fn a_removal_is_a_failure_with_room_in_the_block_or_follows_the_senders_earlier_one() {
        let free = |collated: &[&str], removed: &[&str]| answer(collated, removed, 150, 2_097_152);
        // a failed, and b, its sender's next, went with it, wherever it stands.
        let chained = pool_of(&["a", "s", "b"], &[&["a", "b"]]);
        assert_eq!(checked(&chained, &free(&["s"], &["a", "b"])), Ok(false));
        // b kept after a was removed.
        assert!(checked(&chained, &free(&["s", "b"], &["a"])).is_err());
        // A transaction the byte budget cannot hold stops the collation; it is not tried, so it
        // cannot fail, and nothing after it is collated.
        assert!(
            checked(
                &pool(&["big", "s"]),
                &answer(&["s"], &["big"], 150, 141 + 600)
            )
            .is_err()
        );
        assert_eq!(
            checked(&pool(&["big", "s"]), &answer(&[], &[], 150, 141 + 600)),
            Ok(true)
        );
        // Removed after the block was full by its count: never tried.
        assert!(checked(&pool(&["s", "y"]), &answer(&["s"], &["y"], 1, 2_097_152)).is_err());
    }

    #[test]
    fn a_senders_transactions_keep_their_nonce_order_in_the_priority_order() {
        let free = |collated: &[&str]| answer(collated, &[], 150, 2_097_152);
        // l then m, one sender's: collated in that order.
        let ordered = pool_of(&["s", "l", "m"], &[&["l", "m"]]);
        assert_eq!(checked(&ordered, &free(&["s", "l", "m"])), Ok(false));
        // The priority order puts m, the sender's later transaction, before l: whatever the
        // collation, the pool's order is not the collator's.
        let inverted = pool_of(&["s", "m", "l"], &[&["l", "m"]]);
        assert!(checked(&inverted, &free(&["s", "m", "l"])).is_err());
        assert!(checked(&inverted, &free(&["s", "l", "m"])).is_err());
    }

    #[test]
    fn the_pools_views_must_hold_the_same_transactions() {
        let free = |collated: &[&str], removed: &[&str]| answer(collated, removed, 150, 2_097_152);
        // The priority order leaves out b, which the pool holds.
        let mut short = pool(&["a", "b"]);
        short["highest"] = json!(["a"]);
        assert!(checked(&short, &free(&["a"], &[])).is_err());
        // A removal of a transaction the pool did not hold.
        assert!(checked(&pool(&["a"]), &free(&["a"], &["z"])).is_err());
    }

    #[test]
    fn two_blocks_share_a_verification_by_the_rules_that_fail() {
        let failing = |errors: &[&str]| json!({"verified": false, "errors": errors});
        assert_eq!(
            verdict(&failing(&["Payload is too large: 519 > 500"])),
            verdict(&failing(&["Payload is too large: 529 > 500"]))
        );
        assert_ne!(
            verdict(&failing(&["Payload is too large: 519 > 500"])),
            verdict(&failing(&[
                "Payload is too large: 529 > 500",
                "Failed to verify block signature"
            ]))
        );
        assert_ne!(
            verdict(&json!({"verified": true, "errors": []})),
            verdict(&failing(&["Invalid payload hash"]))
        );
    }

    #[test]
    fn the_collations_differ_only_where_the_pools_do() {
        let free = |collated: &[&str]| answer(collated, &[], 150, 2_097_152);
        // Heartwood's pool also holds l, which it collates after s.
        assert_eq!(
            unexplained(
                &pool(&["s"]),
                &free(&["s"]),
                &pool(&["s", "l"]),
                &free(&["s", "l"])
            ),
            None
        );
        // Heartwood leaves out its own l with room in the block.
        assert!(
            unexplained(
                &pool(&["s"]),
                &free(&["s"]),
                &pool(&["s", "l"]),
                &free(&["s"])
            )
            .is_some()
        );
        // The two collate the transactions both pools hold in another order.
        let both = pool(&["s", "t"]);
        assert!(
            unexplained(
                &both,
                &free(&["s", "t"]),
                &pool(&["t", "s"]),
                &free(&["t", "s"])
            )
            .is_some()
        );
        // One transaction a block: the reference's x, which only its pool holds, takes the place
        // of s, which Heartwood collates.
        let one = |collated: &[&str]| answer(collated, &[], 1, 2_097_152);
        assert_eq!(
            unexplained(
                &pool(&["x", "s"]),
                &one(&["x"]),
                &pool(&["s"]),
                &one(&["s"])
            ),
            None
        );
    }

    #[test]
    fn a_limit_does_not_excuse_a_shared_transaction_both_collators_reached_and_decided_otherwise() {
        let one = |collated: &[&str], removed: &[&str]| answer(collated, removed, 1, 2_097_152);
        // One transaction a block. The reference's pool holds s and t, Heartwood's also l (the
        // documented eviction's). The reference collates s; Heartwood removes s, valid and
        // shared, collates l and stops before t. Each collation follows from its own pool, but
        // both collators reached s and decided it otherwise.
        let (theirs, ours) = (pool(&["s", "t"]), pool(&["s", "l", "t"]));
        assert_eq!(checked(&theirs, &one(&["s"], &[])), Ok(true));
        assert_eq!(checked(&ours, &one(&["l"], &["s"])), Ok(true));
        let found = unexplained(&theirs, &one(&["s"], &[]), &ours, &one(&["l"], &["s"]));
        assert!(
            matches!(&found, Some(Difference::Value { path, expected, actual })
                if path.starts_with("$.collated and $.removed: s, ")
                    && *expected == json!("collated") && *actual == json!("removed")),
            "{found:?}"
        );
        // Where l comes first in Heartwood's priority order, its collator stops before s: s is
        // beyond its limit, so it decided nothing about it.
        assert_eq!(
            unexplained(
                &theirs,
                &one(&["s"], &[]),
                &pool(&["l", "s", "t"]),
                &one(&["l"], &[])
            ),
            None
        );
        // Two a block: the two collate s, Heartwood then its own l, the reference t, which
        // Heartwood's collator did not reach.
        let two = |collated: &[&str]| answer(collated, &[], 2, 2_097_152);
        assert_eq!(
            unexplained(
                &pool(&["s", "t", "u"]),
                &two(&["s", "t"]),
                &pool(&["s", "l", "t", "u"]),
                &two(&["s", "l"])
            ),
            None
        );
        // A shared transaction one side collated and the other removed is no limit's doing.
        let three = |collated: &[&str], removed: &[&str]| answer(collated, removed, 3, 2_097_152);
        assert!(
            unexplained(
                &pool(&["s", "t", "u"]),
                &three(&["s", "t", "u"], &[]),
                &pool(&["s", "l", "t", "u"]),
                &three(&["s", "l", "u"], &["t"])
            )
            .is_some()
        );
    }

    #[test]
    fn a_sender_the_pools_difference_concerns_may_take_another_place_among_equal_fees() {
        let free = |collated: &[&str]| answer(collated, &[], 150, 2_097_152);
        let moved =
            |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| (*id).to_owned()).collect() };
        // The reference evicted t and admitted it again, last among equal fees; Heartwood kept it
        // first. Both pools hold s and t and both collate them.
        let (theirs, ours) = (pool(&["s", "t"]), pool(&["t", "s"]));
        assert_eq!(
            unexplained_collation(
                &theirs,
                &free(&["s", "t"]),
                &ours,
                &free(&["t", "s"]),
                &moved(&["t"])
            ),
            None
        );
        // No sender of the two is concerned: the order is the collators' own.
        assert!(
            unexplained_collation(
                &theirs,
                &free(&["s", "t"]),
                &ours,
                &free(&["t", "s"]),
                &moved(&["u"])
            )
            .is_some()
        );
        // A third sender's place may differ, the order of the others may not.
        let (theirs, ours) = (pool(&["s", "t", "u"]), pool(&["u", "t", "s"]));
        assert!(
            unexplained_collation(
                &theirs,
                &free(&["s", "t", "u"]),
                &ours,
                &free(&["u", "t", "s"]),
                &moved(&["u"])
            )
            .is_some()
        );
        // The moved transaction is still decided alike: collated on one side, removed on the other
        // fails.
        assert!(
            unexplained_collation(
                &pool(&["s", "t"]),
                &free(&["s", "t"]),
                &pool(&["t", "s"]),
                &answer(&["s"], &["t"], 150, 2_097_152),
                &moved(&["t"])
            )
            .is_some()
        );
    }

    #[test]
    fn the_senders_a_pools_difference_concerns_are_the_evicted_ones_and_those_held_on_one_side() {
        // Senders A (a1, a2), B (b1), C (c1) and D (d1). The reference's full pool evicted a1 at
        // step 0; after step 1 only Heartwood's pool holds c1; B and D are held alike.
        let step = |by_sender: Value, all: &[&str], notes: Value| json!({"result": {NOTES: notes}, "contents": {"all": all, "bySender": by_sender}});
        let evicted = json!({"evictions": [{"id": "x", "evicted": ["a1"]}]});
        let theirs = json!({"results": [
            step(json!([["B", ["b1"]], ["D", ["d1"]]]), &["b1", "d1"], evicted),
            step(json!([["B", ["b1"]], ["D", ["d1"]], ["A", ["a2"]]]), &["b1", "d1", "a2"], json!({})),
        ]});
        let ours = json!({"results": [
            step(json!([["A", ["a1"]], ["B", ["b1"]], ["D", ["d1"]]]), &["a1", "b1", "d1"], json!({})),
            step(
                json!([["A", ["a1", "a2"]], ["B", ["b1"]], ["D", ["d1"]], ["C", ["c1"]]]),
                &["a1", "a2", "b1", "d1", "c1"],
                json!({})
            ),
        ]});
        let moved: Vec<String> = moved_by_difference(&theirs, &ours, 2).into_iter().collect();
        assert_eq!(moved, ["a1", "a2", "c1"]);
        // The collation's own step is left out: c1 differs only after step 1.
        let moved: Vec<String> = moved_by_difference(&theirs, &ours, 1).into_iter().collect();
        assert_eq!(moved, ["a1", "a2"]);
        // Without the eviction note and with the same transactions on both sides, none.
        let plain = json!({"results": [step(json!([["B", ["b1"]]]), &["b1"], json!({}))]});
        assert!(moved_by_difference(&plain, &plain, 1).is_empty());
    }
}
