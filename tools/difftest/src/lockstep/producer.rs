//! The producer's loop: a chain forged slot by slot, every transaction judged by both pools and
//! every block applied by both states.

use std::collections::{BTreeSet, HashMap};

use heartwood_crypto::identities::{Address, PublicKeyBytes};
use heartwood_crypto::utils::hex;
use heartwood_kernel::calculate_round;
use heartwood_kernel::utils::round_calculator::is_new_round;
use heartwood_state::digest::Step as Apply;
use serde_json::{Value, json};

use super::LockstepError;
use super::heart::{Heart, comparable};
use super::labels::Labels;
use super::recipe::{Attested, BuildSpec, Recipe, Step};
use super::reference::Reference;
use crate::compare;
use crate::corpus::RevertStep;

/// The time between two submissions, in milliseconds: past the processor's cooldown.
const SUBMIT_GAP_MS: i64 = 31_000;

/// A block of the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord {
    /// The height.
    pub height: u32,
    /// The id, in hex.
    pub id: String,
    /// The block's bytes, in hex.
    pub hex: String,
    /// The number of transactions.
    pub transactions: usize,
    /// The slots missed before the block.
    pub missed: usize,
}

/// What the loop made.
#[derive(Debug, Default)]
pub struct Made {
    /// The blocks after the genesis block, in height order.
    pub blocks: Vec<BlockRecord>,
    /// The reverts the chain went through, for the corpus's revert plan.
    pub reverts: Vec<RevertStep>,
    /// The restart points of the recipe.
    pub restarts: Vec<u32>,
}

impl Made {
    /// The slots missed in the chain made: those before its blocks, not before the blocks a fork
    /// replaced.
    pub fn missed(&self) -> u64 {
        self.blocks.iter().map(|b| b.missed as u64).sum()
    }
}

/// A transaction submitted, and what the recipe expects of the pools.
struct Sent {
    id: String,
    refused: Option<String>,
}

fn block_error(height: u32, detail: String) -> LockstepError {
    LockstepError::Block { height, detail }
}

/// The differences of two digest lines, without their running digests, as text; empty when the
/// lines are equal.
fn line_differences(heartwood: &Value, reference: &Value) -> String {
    compare::diff(&comparable(reference), &comparable(heartwood))
        .iter()
        .take(3)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A pool result as the two pools are compared: the ids of each list and each error's code.
fn verdict(result: &Value) -> Value {
    let errors: serde_json::Map<String, Value> = result["errors"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(id, e)| (id.clone(), e["type"].clone()))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "accept": result["accept"], "broadcast": result["broadcast"],
        "invalid": result["invalid"], "excess": result["excess"], "errors": errors,
    })
}

/// The end of the message of a vote or revoke for a delegate that is not online.
const OFFLINE: &str = "is not operating a node on the network";

/// Whether the pool's error `error` (`{type, message}`) is the refusal `want` names: an error
/// code, or `ERR_OFFLINE` for the refusal of a vote for a delegate that is not attested (the
/// processor reports it as `ERR_APPLY` with the offline message).
fn refusal_is(error: &Value, want: &str) -> bool {
    if want == "ERR_OFFLINE" {
        return error["type"] == "ERR_APPLY"
            && error["message"]
                .as_str()
                .is_some_and(|m| m.ends_with(OFFLINE));
    }
    error["type"] == want
}

/// The chain being made.
pub struct Producer<'a> {
    heart: Heart,
    reference: &'a mut dyn Reference,
    labels: Labels,
    recipe: &'a Recipe,
    aux: [u8; 32],
    clock: i64,
    nonces: HashMap<Address, u64>,
    made: Made,
}

impl<'a> Producer<'a> {
    /// A producer of the chain of `heart` with `reference` beside it; `clock` is the first time
    /// of a submission, in milliseconds.
    pub fn new(
        heart: Heart,
        reference: &'a mut dyn Reference,
        labels: Labels,
        recipe: &'a Recipe,
        aux: [u8; 32],
        clock: i64,
    ) -> Self {
        Producer {
            heart,
            reference,
            labels,
            recipe,
            aux,
            clock,
            nonces: HashMap::new(),
            made: Made::default(),
        }
    }

    /// Check the genesis lines, then grow the chain to `tip`.
    pub fn run(mut self, genesis_line: &Value, tip: u32) -> Result<Made, LockstepError> {
        let ours = self.heart.genesis_line();
        let differences = line_differences(&ours, genesis_line);
        if !differences.is_empty() {
            return Err(block_error(
                1,
                format!("the genesis states differ: {differences}"),
            ));
        }
        while self.heart.height() < tip {
            let next = self.heart.height() + 1;
            self.round_start(next)?;
            self.transactions_at(next)?;
            let bytes = self.forge(0)?;
            self.commit(&bytes, Apply::Apply, "apply")?;
            self.after(next)?;
        }
        Ok(self.made)
    }

    fn stops(&self, name: &str, height: u32) -> bool {
        self.recipe.steps.iter().any(|s| {
            matches!(s, Step::Missed { delegate, from_height, to_height }
                if delegate == name && (*from_height..=*to_height).contains(&height))
        })
    }

    /// The bytes of the block at the next height, in the first slot at least `gap` slots on.
    fn forge(&mut self, gap: u32) -> Result<Vec<u8>, LockstepError> {
        let next = self.heart.height() + 1;
        let slot = self.heart.slot(gap, |name, key| {
            self.labels.forger(key).is_none() || self.stops(name, next)
        })?;
        let key = self.labels.forger(&slot.public_key).ok_or_else(|| {
            LockstepError::Heartwood(format!("no key forges for {}", slot.username))
        })?;
        let transactions = self.heart.candidates()?;
        let block = self.heart.forge(&slot, key, self.aux, &transactions)?;
        Ok(block.encode())
    }

    /// Apply the block `bytes` on both sides, compare their lines, and record the block.
    fn commit(&mut self, bytes: &[u8], step: Apply, name: &str) -> Result<(), LockstepError> {
        let posted = self.heart.apply(bytes, step)?;
        let height = self.heart.height();
        let block_hex = hex::encode(bytes);
        let answer = self.reference.apply(&block_hex, name, posted.expire_at)?;
        if !answer.accepted {
            return Err(block_error(
                height,
                format!(
                    "the reference refuses the block: {}",
                    answer.refusal.unwrap_or_default()
                ),
            ));
        }
        let line = answer.line.unwrap_or(Value::Null);
        let differences = line_differences(&posted.line, &line);
        if !differences.is_empty() {
            return Err(block_error(height, differences));
        }
        if step == Apply::Apply {
            self.made.blocks.push(BlockRecord {
                height,
                id: self.heart.state.last_block().id().to_hex(),
                hex: block_hex,
                transactions: posted.transactions,
                missed: posted.missed,
            });
        }
        Ok(())
    }

    fn key_of(&self, name: &str) -> Result<PublicKeyBytes, LockstepError> {
        self.labels
            .delegate(name)
            .or_else(|| self.heart.delegate_key(name))
            .ok_or_else(|| LockstepError::Recipe(format!("no delegate is named {name}")))
    }

    /// The online steps of the round that starts with the block at `next`.
    fn round_start(&mut self, next: u32) -> Result<(), LockstepError> {
        if next != 2 && !is_new_round(next, &self.heart.milestones) {
            return Ok(());
        }
        let round = calculate_round(next, &self.heart.milestones).round;
        for step in &self.recipe.steps {
            let Step::Online {
                round: r,
                delegates,
            } = step
            else {
                continue;
            };
            if *r != round {
                continue;
            }
            let all = self.heart.forging_keys();
            let keys: Vec<PublicKeyBytes> = match delegates {
                Attested::All => all,
                Attested::AllBut(names) => {
                    let out: Vec<PublicKeyBytes> = names
                        .iter()
                        .map(|n| self.key_of(n))
                        .collect::<Result<_, _>>()?;
                    all.into_iter().filter(|k| !out.contains(k)).collect()
                }
                Attested::Only(names) => names
                    .iter()
                    .map(|n| self.key_of(n))
                    .collect::<Result<_, _>>()?,
            };
            self.heart.set_online(round, &keys);
            let texts: Vec<String> = keys.iter().map(|k| k.to_hex()).collect();
            self.reference.online(round, &texts)?;
        }
        Ok(())
    }

    /// The reference's spec of `spec`, its labels resolved and its nonce chosen.
    fn spec(
        &self,
        spec: &BuildSpec,
        local: &mut HashMap<Address, u64>,
    ) -> Result<Value, LockstepError> {
        let sender = self.labels.person(&spec.sender)?;
        let base = local.get(&sender.address).copied().unwrap_or_else(|| {
            let known = self.nonces.get(&sender.address).copied().unwrap_or(0);
            self.heart.nonce(&sender.address).max(known)
        });
        let nonce = base + 1;
        if spec.refused.is_none() {
            local.insert(sender.address, nonce);
        }
        let mut asset = spec.asset.clone();
        if spec.kind == "transfer" {
            let recipients = asset["recipients"].as_array().cloned().unwrap_or_default();
            let mut resolved = Vec::new();
            for pair in recipients {
                let label = pair[0].as_str().unwrap_or_default();
                let address = self.labels.person(label)?.address.to_string();
                resolved.push(json!([address, pair[1]]));
            }
            asset["recipients"] = Value::Array(resolved);
        }
        let mut out = json!({
            "kind": spec.kind, "passphrase": sender.passphrase, "nonce": nonce.to_string(),
            "asset": asset,
        });
        if let Some(fee) = &spec.fee {
            out["fee"] = json!(fee);
        }
        if let Some(memo) = &spec.memo {
            out["memo"] = json!(memo);
        }
        Ok(out)
    }

    /// Build the transactions of the recipe's steps at `next`, and give both pools them.
    fn transactions_at(&mut self, next: u32) -> Result<(), LockstepError> {
        for step in &self.recipe.steps {
            let Step::Transactions { height, build } = step else {
                continue;
            };
            if *height != next || build.is_empty() {
                continue;
            }
            let mut local = HashMap::new();
            let specs: Vec<Value> = build
                .iter()
                .map(|b| self.spec(b, &mut local))
                .collect::<Result<_, _>>()?;
            let built = self.reference.build(next, &specs)?;
            if built.len() != build.len() {
                return Err(LockstepError::Oracle(
                    "the reference built another number of transactions".into(),
                ));
            }
            let sent: Vec<Sent> = built
                .iter()
                .zip(build)
                .map(|(t, b)| Sent {
                    id: t.id.clone(),
                    refused: b.refused.clone(),
                })
                .collect();
            let bytes: Vec<Vec<u8>> = built
                .iter()
                .map(|t| {
                    hex::decode(&t.hex).map_err(|e| {
                        LockstepError::Oracle(format!("transaction {} is not hex: {e}", t.id))
                    })
                })
                .collect::<Result<_, _>>()?;
            let hexes: Vec<String> = built.iter().map(|t| t.hex.clone()).collect();
            let now = self.clock;
            self.clock += SUBMIT_GAP_MS;
            let ours = self.heart.submit(now, bytes)?;
            let theirs = self.reference.submit(now, &hexes)?;
            self.judge(&sent, &ours, &theirs)?;
            for b in build {
                if b.refused.is_none() {
                    let address = self.labels.person(&b.sender)?.address;
                    let nonce = local.get(&address).copied().unwrap_or(0);
                    self.nonces.insert(address, nonce);
                }
            }
        }
        Ok(())
    }

    /// Compare the pools' verdicts on `sent`, and them with the recipe's.
    fn judge(&self, sent: &[Sent], ours: &Value, theirs: &Value) -> Result<(), LockstepError> {
        let (a, b) = (verdict(ours), verdict(theirs));
        let status = |v: &Value, id: &str| -> String {
            let lists: Vec<&str> = ["accept", "broadcast", "invalid", "excess"]
                .into_iter()
                .filter(|k| v[*k].as_array().is_some_and(|l| l.iter().any(|i| i == id)))
                .collect();
            format!("{lists:?} {}", v["errors"][id])
        };
        if a != b {
            let id = sent
                .iter()
                .find(|s| status(&a, &s.id) != status(&b, &s.id))
                .map_or("(the batch)", |s| s.id.as_str());
            return Err(LockstepError::Pool {
                id: id.to_owned(),
                detail: format!(
                    "Heartwood answers {} and the reference {}",
                    if id == "(the batch)" {
                        a.to_string()
                    } else {
                        status(&a, id)
                    },
                    if id == "(the batch)" {
                        b.to_string()
                    } else {
                        status(&b, id)
                    },
                ),
            });
        }
        for s in sent {
            let accepted = b["accept"]
                .as_array()
                .is_some_and(|l| l.iter().any(|i| *i == s.id));
            let fine = match &s.refused {
                None => accepted,
                Some(want) => {
                    !accepted
                        && [ours, theirs]
                            .iter()
                            .all(|r| refusal_is(&r["errors"][&s.id], want))
                }
            };
            if !fine {
                return Err(LockstepError::Pool {
                    id: s.id.clone(),
                    detail: format!(
                        "the recipe expects {}, both pools answer {} ({})",
                        s.refused.as_deref().unwrap_or("acceptance"),
                        status(&b, &s.id),
                        theirs["errors"][&s.id]["message"]
                            .as_str()
                            .unwrap_or("no message"),
                    ),
                });
            }
        }
        Ok(())
    }

    /// The recipe's reverts, forks and restarts after the block at `height`.
    fn after(&mut self, height: u32) -> Result<(), LockstepError> {
        for step in &self.recipe.steps {
            match step {
                Step::Revert {
                    after_height,
                    blocks,
                } if *after_height == height => {
                    self.revert(*blocks)?;
                    self.made.reverts.push(RevertStep {
                        after_height: height,
                        blocks: *blocks,
                    });
                    self.reapply(*blocks)?;
                }
                Step::Fork {
                    after_height,
                    blocks,
                } if *after_height == height => {
                    self.revert(*blocks)?;
                    let keep = self.made.blocks.len() - *blocks as usize;
                    self.made.blocks.truncate(keep);
                    for i in 0..*blocks {
                        let bytes = self.forge(u32::from(i == 0))?;
                        self.commit(&bytes, Apply::Apply, "apply")?;
                    }
                }
                Step::Restart { height: at } if *at == height => self.made.restarts.push(height),
                _ => {}
            }
        }
        Ok(())
    }

    /// Revert `count` blocks on both sides and compare their lines.
    fn revert(&mut self, count: u32) -> Result<(), LockstepError> {
        let tip = self.heart.height();
        let (ours, _) = self.heart.revert(count)?;
        let theirs = self.reference.revert(count)?;
        if ours.len() != theirs.len() {
            return Err(block_error(
                tip,
                format!(
                    "the reference answers {} lines of a {count}-block revert",
                    theirs.len()
                ),
            ));
        }
        for (i, (a, b)) in ours.iter().zip(&theirs).enumerate() {
            let differences = line_differences(a, b);
            if !differences.is_empty() {
                return Err(block_error(
                    tip - i as u32 - 1,
                    format!("the revert: {differences}"),
                ));
            }
        }
        Ok(())
    }

    /// Apply the last `count` blocks of the chain again, on both sides.
    fn reapply(&mut self, count: u32) -> Result<(), LockstepError> {
        let from = self.made.blocks.len() - count as usize;
        let again: Vec<String> = self.made.blocks[from..]
            .iter()
            .map(|b| b.hex.clone())
            .collect();
        for text in again {
            let bytes = hex::decode(&text)
                .map_err(|e| LockstepError::Heartwood(format!("a recorded block: {e}")))?;
            self.commit(&bytes, Apply::Reapply, "reapply")?;
        }
        Ok(())
    }
}

/// Refuse a recipe with a step beyond a chain of `rounds` rounds and `tip` blocks, a step that
/// cannot run, or a revert back to the last block of a round.
pub fn check(recipe: &Recipe, rounds: u32, tip: u32) -> Result<(), LockstepError> {
    let bad = |m: String| Err(LockstepError::Recipe(m));
    let round_length = tip / rounds.max(1);
    let mut rounds_seen = BTreeSet::new();
    for step in &recipe.steps {
        match step {
            Step::Transactions { height, .. } if !(2..=tip).contains(height) => {
                return bad(format!(
                    "a transactions step at height {height}: the chain has {tip} blocks"
                ));
            }
            Step::Missed {
                from_height,
                to_height,
                ..
            } if from_height > to_height || *to_height > tip => {
                return bad(format!(
                    "a missed step from {from_height} to {to_height}: the chain has {tip} blocks"
                ));
            }
            Step::Online { round, .. } => {
                if *round == 0 || *round > u64::from(rounds) {
                    return bad(format!(
                        "an online step of round {round}: the chain has {rounds} rounds"
                    ));
                }
                if !rounds_seen.insert(*round) {
                    return bad(format!("two online steps of round {round}"));
                }
            }
            Step::Revert {
                after_height,
                blocks,
            }
            | Step::Fork {
                after_height,
                blocks,
            } if *blocks == 0 || *after_height > tip || *blocks >= *after_height => {
                return bad(format!(
                    "a revert or fork of {blocks} blocks after height {after_height}"
                ));
            }
            // The plan's revert runs again in every replay of the corpus, and a reference node that
            // reverts to a round's last block deletes the next round's rows without saving them
            // again, so each later line of the round would differ on the plan, not on the chain.
            Step::Revert {
                after_height,
                blocks,
            } if round_length > 0 && (after_height - blocks) % round_length == 0 => {
                return bad(format!(
                    "a revert of {blocks} blocks after height {after_height} returns to the last \
                     block of a round; revert one block more or one fewer"
                ));
            }
            Step::Restart { height } if !(1..=tip).contains(height) => {
                return bad(format!(
                    "a restart at height {height}: the chain has {tip} blocks"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}
