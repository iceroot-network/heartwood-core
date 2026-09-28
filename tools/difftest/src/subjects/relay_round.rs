//! The relay's round and slot answers for its forger, answered as the internal controller answers
//! them over a node of a corpus chain:
//!
//! - `forger.round` `{chain: {crypto, blocks}, at: [{now, timestamps?}]}` -> `{answers: [{round,
//!   slotNumbers}]}`: a node started on the chain definition `crypto` with the blocks of `blocks`
//!   applied, then at each moment getCurrentRound's answer (its text) and getSlotNumber for each
//!   of the timestamps (see `tools/oracle/lib/ops/relay.js` for the input).
//!
//! The node is the job harness's ([`JobRig`]): the chain definition's first start, the blocks
//! applied and committed to its store. Its state, pool and stored rounds go through the functions
//! a relay's node services answer with ([`current_round`] and [`slot_number`]), and the round is
//! written by the internal routes' codec, as the relay's controller writes it.
//!
//! The wallets in the answer are a documented difference ([`Documented::RoundWallets`]): the
//! reference writes its wallet data and lists `allDelegates` in the order its name index was
//! filled, Heartwood writes the API's wallet shape and lists them by name. [`masked`] takes
//! Heartwood's wallets into the reference's answer wherever the key and the name agree, so every
//! other field, the order of the round's delegates and the two forgers are compared exactly.

use std::collections::BTreeMap;

use heartwood_core::node_services::{current_round, slot_number};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, PublicKeyBytes};
use heartwood_kernel::RoundInfo;
use heartwood_p2p::errors::LookupError;
use heartwood_p2p_wire::internal;
use serde_json::{Value, json};

use super::{Env, Outcome, Run, failed};
use crate::levels::l2_processor::{JobRig, crypto_of_json};
use crate::store::StartProblem;
use crate::subject::{Case, Documented, SubjectError, Verdict};

/// The operations of the relay's round answers.
pub(super) const OPS: &[(&str, Run)] = &[("forger.round", round)];

/// The fields of a round answer that hold one wallet each.
const WALLETS: [&str; 2] = ["currentForger", "nextForger"];

fn start(problem: StartProblem) -> SubjectError {
    match problem {
        StartProblem::Unbuilt(_) => SubjectError::NotImplemented,
        StartProblem::Failed(why) => failed(why),
    }
}

fn round(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let chain = &case.input["chain"];
    let files = crypto_of_json(&chain["crypto"], None).map_err(start)?;
    let mut rig = JobRig::start(&files).map_err(start)?;
    let hexes = chain["blocks"]
        .as_array()
        .ok_or_else(|| failed("chain.blocks must be a list"))?;
    let blocks = hexes
        .iter()
        .enumerate()
        .map(|(i, hex)| {
            let bytes = hex::decode(hex.as_str().unwrap_or_default())
                .map_err(|e| failed(format!("chain.blocks[{i}]: {e}")))?;
            Block::receive(&bytes, rig.milestones())
                .map_err(|e| failed(format!("chain.blocks[{i}]: {e}")))
        })
        .collect::<Outcome<Vec<_>>>()?;
    rig.extend(&blocks).map_err(start)?;
    let (state, pool) = (rig.state().view(), rig.pool().view());
    // The stored round's delegates, in rank order, as the relay's view reads them after a commit.
    let stored = |round: RoundInfo| -> Result<Vec<PublicKeyBytes>, LookupError> {
        let fail = |e: &dyn std::fmt::Display| LookupError(e.to_string());
        let rows = rig.store().database().round(round.round);
        let rows = rows.map_err(|e| fail(&e))?;
        let key = |text: &str| PublicKeyBytes::from_hex(text).map_err(|e| fail(&e));
        rows.iter().map(|row| key(&row.public_key)).collect()
    };
    let moments = case.input["at"]
        .as_array()
        .ok_or_else(|| failed("at must be a list"))?;
    let mut answers = Vec::new();
    for (i, moment) in moments.iter().enumerate() {
        let now = moment["now"]
            .as_i64()
            .ok_or_else(|| failed(format!("at[{i}].now is not an integer")))?;
        let answer = current_round(&state, &pool, rig.milestones(), now, stored)
            .map_err(|e| failed(format!("at[{i}]: getCurrentRound: {e}")))?;
        let text = String::from_utf8(internal::encode(&answer))
            .map_err(|e| failed(format!("at[{i}]: the round's text: {e}")))?;
        let timestamps = match &moment["timestamps"] {
            Value::Null => &Vec::new(),
            Value::Array(timestamps) => timestamps,
            _ => return Err(failed(format!("at[{i}].timestamps must be a list"))),
        };
        let slot = |timestamp: &Value| match timestamp {
            Value::Null => Ok(slot_number(rig.milestones(), state.height(), None, now)),
            other => other
                .as_i64()
                .map(|t| slot_number(rig.milestones(), state.height(), Some(t), now))
                .ok_or_else(|| failed(format!("at[{i}].timestamps holds {other}"))),
        };
        let slots = timestamps.iter().map(slot).collect::<Outcome<Vec<_>>>()?;
        answers.push(json!({ "round": text, "slotNumbers": slots }));
    }
    Ok(Verdict::DiffersWhereShown {
        difference: Documented::RoundWallets,
        verdict: Box::new(Verdict::Output(json!({ "answers": answers }))),
    })
}

/// A wallet's key and delegate name, what the forger reads of it.
fn identity(wallet: &Value) -> Option<(&str, &str)> {
    let key = wallet["publicKey"].as_str()?;
    Some((key, wallet["delegate"]["username"].as_str()?))
}

/// The keys and names of a round answer's `allDelegates`, with how often each is listed.
fn listed(round: &Value) -> Option<BTreeMap<(&str, &str), usize>> {
    let mut counts = BTreeMap::new();
    for wallet in round["allDelegates"].as_array()? {
        *counts.entry(identity(wallet)?).or_insert(0_usize) += 1;
    }
    Some(counts)
}

/// The reference's round answer `theirs` with the wallets of Heartwood's `ours` wherever the
/// key and the name agree: each wallet of `delegates`, `currentForger` and `nextForger` at its
/// place, and `allDelegates` whole when both list the same keys and names (in any order).
fn with_our_wallets(theirs: &Value, ours: &Value) -> Value {
    let mut adjusted = theirs.clone();
    let same = |a: &Value, b: &Value| identity(a).is_some() && identity(a) == identity(b);
    for field in WALLETS {
        if same(&theirs[field], &ours[field]) {
            adjusted[field] = ours[field].clone();
        }
    }
    if let (Some(delegates), Some(our_delegates)) =
        (theirs["delegates"].as_array(), ours["delegates"].as_array())
    {
        for (at, (wallet, our_wallet)) in delegates.iter().zip(our_delegates).enumerate() {
            if same(wallet, our_wallet) {
                adjusted["delegates"][at] = our_wallet.clone();
            }
        }
    }
    if listed(theirs).is_some() && listed(theirs) == listed(ours) {
        adjusted["allDelegates"] = ours["allDelegates"].clone();
    }
    adjusted
}

/// The oracle's `forger.round` output `theirs` with Heartwood's wallets taken into each answer
/// whose round, so adjusted, equals Heartwood's round in `ours` (the answer then takes
/// Heartwood's text); `None` when no answer has such a round. An answer whose other fields
/// differ keeps the reference's text, and fails the comparison.
pub fn masked(theirs: &Value, ours: &Value) -> Option<Value> {
    let (answers, our_answers) = (theirs["answers"].as_array()?, ours["answers"].as_array()?);
    let mut adjusted = theirs.clone();
    let mut changed = false;
    for (at, (answer, our_answer)) in answers.iter().zip(our_answers).enumerate() {
        let parse = |answer: &Value| serde_json::from_str::<Value>(answer["round"].as_str()?).ok();
        let (Some(round), Some(our_round)) = (parse(answer), parse(our_answer)) else {
            continue;
        };
        if with_our_wallets(&round, &our_round) == our_round {
            adjusted["answers"][at]["round"] = our_answer["round"].clone();
            changed = true;
        }
    }
    changed.then_some(adjusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet(key: &str, name: &str, balance: &str) -> Value {
        json!({ "publicKey": key, "balance": balance, "delegate": { "username": name } })
    }

    fn output(round: &Value) -> Value {
        json!({ "answers": [{ "round": round.to_string(), "slotNumbers": [1] }] })
    }

    fn round(all: Vec<Value>, delegates: Vec<Value>, reward: &str) -> Value {
        json!({
            "allDelegates": all,
            "currentForger": delegates[1].clone(),
            "delegates": delegates,
            "reward": reward,
        })
    }

    #[test]
    fn only_the_wallets_fields_and_the_order_of_all_delegates_are_taken() {
        let (a, b) = (wallet("02aa", "a", "1"), wallet("02bb", "b", "2"));
        let (our_a, our_b) = (wallet("02aa", "a", "9"), wallet("02bb", "b", "8"));
        let theirs = round(vec![b.clone(), a.clone()], vec![a.clone(), b.clone()], "5");
        let ours = round(
            vec![our_a.clone(), our_b.clone()],
            vec![our_a.clone(), our_b.clone()],
            "5",
        );
        let taken = masked(&output(&theirs), &output(&ours)).unwrap();
        assert_eq!(taken, output(&ours));

        // Another order of the round's delegates, a forger or any other field is not taken.
        for other in [
            round(
                vec![our_a.clone(), our_b.clone()],
                vec![our_b.clone(), our_a.clone()],
                "5",
            ),
            round(
                vec![our_a.clone(), our_b.clone()],
                vec![our_a.clone(), our_b.clone()],
                "6",
            ),
            round(vec![our_a.clone()], vec![our_a.clone(), our_b.clone()], "5"),
        ] {
            assert_eq!(masked(&output(&theirs), &output(&other)), None);
        }
        let mut renamed = ours.clone();
        renamed["currentForger"] = wallet("02bb", "c", "8");
        assert_eq!(masked(&output(&theirs), &output(&renamed)), None);
    }
}
