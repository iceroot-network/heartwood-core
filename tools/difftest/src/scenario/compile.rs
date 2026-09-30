//! The scenario compiler: every transaction of a scenario built once through the oracle, so that
//! the implementations under test are given the same bytes and the same ids.
//!
//! Planning. A step's trigger names the tip it fires at ([`fire_heights`]: a height, so many blocks
//! or slots after the previous step, the start or the end of a round; a slot counts as a height,
//! which holds on a chain without missed slots), and a submit's transactions are planned for the
//! block after that tip: the height their fee floor and their strict decode use. A nonce is `next`
//! (one more than the sender's last nonce: its last in the scenario, else the one it reached in the
//! genesis block, else 0) or a number, which the sender's later `next` follows. A fee that is absent is the dynamic floor at the planned height.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::chain::Chain;
use super::format::{Action, Compiled, CompiledTx, Nonce, RoundPosition, Scenario, TxSpec};
use crate::errors::{Error, Result};
use crate::levels::l2::call;
use crate::oracle::{Oracle, OracleServer};

/// The tip each step fires at, in step order; the first step counts from the start (height 1, and
/// `settleBlocks` on).
pub fn fire_heights(scenario: &Scenario, round_length: u32) -> Result<Vec<u32>> {
    let mut previous = 1 + scenario.settle_blocks;
    let mut out = Vec::new();
    for (index, step) in scenario.steps.iter().enumerate() {
        let at = &step.at;
        let bad = |why: &str| Error::Usage(format!("step {}: the trigger {why}", index + 1));
        let round = |round: u64| {
            u32::try_from(round)
                .ok()
                .and_then(|r| r.checked_mul(round_length))
                .ok_or_else(|| bad("names a round beyond the chain"))
        };
        let forms = usize::from(at.height.is_some())
            + usize::from(at.after_blocks.is_some())
            + usize::from(at.slot.is_some())
            + usize::from(at.slots_after.is_some())
            + usize::from(at.round.is_some())
            + usize::from(at.round_end == Some(true));
        if forms != 1 || at.round.is_some() != at.position.is_some() {
            return Err(bad("must be exactly one form (a round with its position)"));
        }
        let fired = if let Some(height) = at.height {
            height
        } else if let Some(blocks) = at.after_blocks.or(at.slots_after) {
            previous + blocks
        } else if let Some(slot) = at.slot {
            u32::try_from(slot).map_err(|_| bad("names a slot beyond the chain"))?
        } else if let (Some(r), Some(position)) = (at.round, at.position) {
            match position {
                RoundPosition::Start => round(r.saturating_sub(1))? + 1,
                RoundPosition::End => round(r)?,
            }
        } else {
            previous.div_ceil(round_length.max(1)).max(1) * round_length
        };
        previous = fired.max(previous);
        out.push(previous);
    }
    Ok(out)
}

/// The oracle's spec of `spec`, its actors resolved to keys and addresses, its nonce `nonce`, for
/// the block at `height`.
fn oracle_spec(chain: &Chain, spec: &TxSpec, nonce: u64, height: u32) -> Result<Value> {
    let sender = chain.person(&spec.from)?;
    let mut asset = spec.asset.clone().unwrap_or_else(|| json!({}));
    match spec.kind.as_str() {
        "transfer" => {
            let mut recipients = Vec::new();
            for (name, amount) in &spec.to {
                recipients.push(json!([chain.person(name)?.address, amount]));
            }
            asset["recipients"] = Value::Array(recipients);
        }
        "secondSignature" => {
            let owner = spec.second_signature.as_deref().unwrap_or(&spec.from);
            asset["secondPassphrase"] = json!(chain.person(owner)?.passphrase);
        }
        "vote" | "delegateRegistration" | "delegateResignation" | "burn" | "ipfs" => {}
        other => {
            return Err(Error::Usage(format!(
                "{}: no such type {other}",
                spec.reference
            )));
        }
    }
    let mut out = json!({
        "kind": spec.kind, "passphrase": sender.passphrase, "nonce": nonce.to_string(),
        "height": height, "asset": asset,
    });
    if let (Some(second), true) = (&spec.second_signature, spec.kind != "secondSignature") {
        out["secondPassphrase"] = json!(chain.person(second)?.passphrase);
    }
    if let Some(fee) = &spec.fee {
        out["fee"] = json!(fee);
    }
    if let Some(memo) = &spec.memo {
        out["memo"] = json!(memo);
    }
    Ok(out)
}

/// The steps' transactions as oracle specs, with the step and planned height of each, in step
/// order.
pub fn specs(scenario: &Scenario, chain: &Chain) -> Result<Vec<(usize, u32, String, Value)>> {
    let fires = fire_heights(scenario, chain.round_length)?;
    let mut nonces: HashMap<&str, u64> = HashMap::new();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (index, (step, fired)) in scenario.steps.iter().zip(fires).enumerate() {
        if !step.txs.is_empty() && step.action != Some(Action::Submit) {
            return Err(Error::Usage(format!(
                "step {}: transactions without a submit",
                index + 1
            )));
        }
        for tx in &step.txs {
            if !seen.insert(tx.reference.as_str()) {
                return Err(Error::Usage(format!("{} is named twice", tx.reference)));
            }
            let last = match nonces.entry(tx.from.as_str()) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(first) => first.insert(chain.genesis_nonce(&tx.from)?),
            };
            let nonce = match tx.nonce {
                Nonce::Next => *last + 1,
                Nonce::At(nonce) => nonce,
            };
            *last = nonce;
            let spec = oracle_spec(chain, tx, nonce, fired + 1)?;
            out.push((index, fired + 1, tx.reference.clone(), spec));
        }
    }
    Ok(out)
}

/// `scenario`'s transactions built through `server`.
pub fn compile_with(scenario: &Scenario, server: &mut OracleServer) -> Result<Compiled> {
    let chain = Chain::new(scenario)?;
    let planned = specs(scenario, &chain)?;
    let args = json!({
        "crypto": chain.crypto,
        "transactions": planned.iter().map(|p| p.3.clone()).collect::<Vec<_>>(),
    });
    let built = call(server, "scenario.compile", 1, &args)?;
    let list = built["transactions"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if list.len() != planned.len() {
        return Err(Error::Oracle(
            "scenario.compile answered another number of transactions".into(),
        ));
    }
    let mut transactions = Vec::new();
    for ((step, height, reference, _), built) in planned.into_iter().zip(list) {
        let (Some(id), Some(bytes)) = (built["id"].as_str(), built["hex"].as_str()) else {
            return Err(Error::Oracle(format!(
                "scenario.compile answered {reference} without id and hex"
            )));
        };
        transactions.push(CompiledTx {
            reference,
            step,
            height,
            id: id.to_owned(),
            bytes: bytes.to_owned(),
        });
    }
    Ok(Compiled {
        scenario: scenario.clone(),
        transactions,
    })
}

/// `scenario`'s transactions built by the reference `oracle` runs.
pub fn compile(scenario: &Scenario, oracle: &Oracle) -> Result<Compiled> {
    compile_with(scenario, &mut oracle.server()?)
}
