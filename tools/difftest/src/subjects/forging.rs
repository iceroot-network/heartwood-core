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
//!   `tools/oracle/lib/ops/forger-forge.js`).
//!
//! The pool session runs through this crate's `pool.run` subject, with the forger's collation as
//! its last step; the collated transactions are found among the steps' entries by their ids. A
//! session in which the reference's full pool evicted a transaction before refusing the new one
//! is the pool's documented difference, and its collation is not compared.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use heartwood_crypto::utils::hex;
use heartwood_crypto::{Aux, BlockId, KeyPair, Milestones, SecretKey, Transaction};
use heartwood_forger::{Delegate, ForgeInputs};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, amount, bytes32, decode_refused, failed, integer, text};
use crate::subject::{Case, Documented, Verdict};

/// The operations of block assembly.
pub(super) const OPS: &[(&str, Run)] = &[
    ("forger.forge", forge),
    ("forger.collateAndForge", collate_and_forge),
];

/// The keys of `forger.collateAndForge`'s input that are the forger's, not the pool session's.
const FORGER_KEYS: [&str; 5] = ["privateKey", "aux", "previousBlock", "timestamp", "reward"];

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
    steps.push(json!({"op": "collate", "validate": true, "exclude": []}));
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
    let verdict = match block {
        Verdict::Output(block) => Verdict::Output(json!({
            "collated": collated, "removed": removed, "hex": block["hex"], "id": block["id"],
        })),
        refused => refused,
    };
    // A session in which the reference's full pool evicted a transaction it then refused holds
    // other transactions in Heartwood's pool, so its collation is not compared.
    Ok(Verdict::DiffersWhereShown {
        difference: Documented::FullPoolEviction,
        verdict: Box::new(verdict),
    })
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
