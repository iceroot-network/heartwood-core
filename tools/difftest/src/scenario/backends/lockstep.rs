//! The lockstep backend: a scenario run through the lockstep producer. The chain is made first,
//! from a recipe the scenario translates to (its transactions are the compiled ones, so the
//! reference and Heartwood are given the bytes the other backends are given), while a recording
//! reference notes each pool verdict and each block applied or reverted. The runner then walks that
//! timeline: a wait moves to the next block, a `revert` or `fork` step takes the reverted blocks
//! back, and the answers to `submit` are the recorded verdicts. There is no network, so a check of
//! the API is skipped there; the producer itself agrees each digest line with the reference.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde_json::Value;

use super::super::backend::{Backend, BackendError, Submitted, Tip};
use super::super::chain::Chain;
use super::super::compile::fire_heights;
use super::super::format::{Action, Compiled, CompiledTx, Part, RestartModes, Via};
use crate::errors::{Error, Result};
use crate::lockstep::reference::{BlockAnswer, BuiltTransaction, Reference};
use crate::lockstep::{Attested, BuildSpec, LockstepError, Recipe, SHORT_ROUNDS, Step, make_with};
use crate::store::ScratchDir;

/// What happened to the chain, in order.
#[derive(Debug, Clone)]
enum Event {
    /// A block was applied.
    Applied {
        height: u32,
        id: String,
        transactions: Vec<String>,
        line: Value,
    },
    /// Blocks were reverted, each with the line of the state it went back to.
    Reverted { lines: Vec<Value> },
}

/// The reference, recorded: what it answers to the producer is what `inner` answers, except that
/// it builds nothing itself (the transactions are the compiled ones).
struct Recording<'a> {
    inner: &'a mut dyn Reference,
    compiled: &'a [CompiledTx],
    queue: HashMap<u32, VecDeque<usize>>,
    verdicts: HashMap<String, (bool, Option<String>, Option<String>)>,
    events: Vec<Event>,
    genesis: Value,
}

fn oracle(what: &str) -> LockstepError {
    LockstepError::Oracle(what.to_owned())
}

impl Reference for Recording<'_> {
    fn open(
        &mut self,
        crypto: &Value,
        options: &Value,
    ) -> std::result::Result<Value, LockstepError> {
        self.genesis = self.inner.open(crypto, options)?;
        Ok(self.genesis.clone())
    }

    fn build(
        &mut self,
        height: u32,
        specs: &[Value],
    ) -> std::result::Result<Vec<BuiltTransaction>, LockstepError> {
        let queue = self.queue.entry(height).or_default();
        (0..specs.len())
            .map(|_| {
                let index = queue.pop_front().ok_or_else(|| {
                    oracle("the recipe builds more transactions than the scenario has")
                })?;
                let tx = &self.compiled[index];
                Ok(BuiltTransaction {
                    id: tx.id.clone(),
                    hex: tx.bytes.clone(),
                })
            })
            .collect()
    }

    fn submit(&mut self, now: i64, hexes: &[String]) -> std::result::Result<Value, LockstepError> {
        let result = self.inner.submit(now, hexes)?;
        for tx in self.compiled.iter().filter(|t| hexes.contains(&t.bytes)) {
            let listed = |list: &str| {
                result[list]
                    .as_array()
                    .is_some_and(|l| l.iter().any(|i| *i == tx.id))
            };
            let error = &result["errors"][&tx.id];
            let error = error.get(0).unwrap_or(error);
            let (code, message) = super::http::refusal(error);
            let accepted = listed("accept") || listed("broadcast");
            self.verdicts.insert(
                tx.id.clone(),
                if accepted {
                    (true, None, None)
                } else {
                    (false, code, message)
                },
            );
        }
        Ok(result)
    }

    fn online(
        &mut self,
        round: u64,
        delegates: &[String],
    ) -> std::result::Result<(), LockstepError> {
        self.inner.online(round, delegates)
    }

    fn apply(
        &mut self,
        hex: &str,
        step: &str,
        expire_at: Option<u64>,
    ) -> std::result::Result<BlockAnswer, LockstepError> {
        let answer = self.inner.apply(hex, step, expire_at)?;
        if let Some(line) = &answer.line {
            let transactions = self
                .compiled
                .iter()
                .filter(|t| hex.contains(&t.bytes))
                .map(|t| t.id.clone())
                .collect();
            self.events.push(Event::Applied {
                height: line["h"]
                    .as_u64()
                    .and_then(|h| u32::try_from(h).ok())
                    .unwrap_or(0),
                id: line["id"].as_str().unwrap_or_default().to_owned(),
                transactions,
                line: line.clone(),
            });
        }
        Ok(answer)
    }

    fn revert(&mut self, count: u32) -> std::result::Result<Vec<Value>, LockstepError> {
        let lines = self.inner.revert(count)?;
        self.events.push(Event::Reverted {
            lines: lines.clone(),
        });
        Ok(lines)
    }

    fn close(&mut self) -> std::result::Result<(), LockstepError> {
        self.inner.close()
    }
}

/// One block of the chain as far as the runner has walked.
struct Held {
    height: u32,
    id: String,
    transactions: Vec<String>,
}

/// A chain the producer made, walked by the runner.
pub struct Lockstep {
    events: Vec<Event>,
    verdicts: HashMap<String, (bool, Option<String>, Option<String>)>,
    position: usize,
    chain: Vec<Held>,
    lines: Vec<Value>,
}

fn unsupported(action: &'static str) -> BackendError {
    BackendError::Unsupported {
        backend: "lockstep",
        action,
    }
}

fn usage(text: String) -> Error {
    Error::Usage(format!("lockstep: {text}"))
}

/// The recipe of `compiled` and the rounds of the chain it needs.
pub fn recipe(compiled: &Compiled, chain: &Chain) -> Result<(Recipe, u32)> {
    let scenario = &compiled.scenario;
    let fires = fire_heights(scenario, chain.round_length)?;
    let mut steps = Vec::new();
    let mut needed = 1;
    for (index, (step, fire)) in scenario.steps.iter().zip(&fires).enumerate() {
        let after = step
            .check
            .as_ref()
            .and_then(|c| c.within_blocks)
            .unwrap_or(0);
        needed = needed.max(fire + after);
        match step.action {
            None => {}
            Some(Action::Submit) => {
                let mut build = Vec::new();
                for spec in &step.txs {
                    let refused = step
                        .check
                        .as_ref()
                        .and_then(|c| c.codes.get(&spec.reference).cloned());
                    let expected_refusal = step
                        .check
                        .as_ref()
                        .is_some_and(|c| c.invalid.contains(&spec.reference));
                    if expected_refusal && refused.is_none() {
                        return Err(usage(format!(
                            "{} is expected invalid: name its refusal code in check.codes",
                            spec.reference
                        )));
                    }
                    // A second-signature registration names the key it registers, and its
                    // compiled bytes are what the pools are given; a transaction signed with a
                    // second key is another matter.
                    if spec.second_signature.is_some() && spec.kind != "secondSignature" {
                        return Err(usage(format!(
                            "{} is second-signed, which a lockstep chain cannot build",
                            spec.reference
                        )));
                    }
                    let asset = if spec.kind == "transfer" {
                        let to: Vec<Value> = spec
                            .to
                            .iter()
                            .map(|(who, amount)| serde_json::json!([chain.label(who), amount]))
                            .collect();
                        serde_json::json!({ "recipients": to })
                    } else {
                        spec.asset.clone().unwrap_or(Value::Null)
                    };
                    build.push(BuildSpec {
                        kind: spec.kind.clone(),
                        sender: chain.label(&spec.from),
                        asset,
                        fee: spec.fee.clone(),
                        memo: spec.memo.clone(),
                        refused,
                    });
                }
                steps.push(Step::Transactions {
                    height: fire + 1,
                    build,
                });
                needed = needed.max(fire + 1 + after);
            }
            Some(Action::Revert | Action::Fork) => {
                let blocks = step
                    .blocks
                    .ok_or_else(|| usage(format!("step {} names no blocks", index + 1)))?;
                let after_height = *fire;
                steps.push(if step.action == Some(Action::Revert) {
                    Step::Revert {
                        after_height,
                        blocks,
                    }
                } else {
                    Step::Fork {
                        after_height,
                        blocks,
                    }
                });
            }
            Some(Action::Wait) => needed = needed.max(fire + step.blocks.unwrap_or(0)),
            Some(other) => {
                return Err(usage(format!(
                    "step {} does {other:?}, which a lockstep chain cannot",
                    index + 1
                )));
            }
        }
    }
    // One block more than the last one wanted, which a check after the last step may see.
    let rounds = (needed + 1).div_ceil(chain.round_length.max(1)).max(1);
    if rounds > SHORT_ROUNDS {
        return Err(usage(format!(
            "the scenario needs {rounds} rounds, and a lockstep chain has at most {SHORT_ROUNDS}"
        )));
    }
    for round in 1..=u64::from(rounds) {
        steps.push(Step::Online {
            round,
            delegates: Attested::All,
        });
    }
    Ok((
        Recipe {
            wallets: chain.actor_labels(),
            steps,
        },
        rounds,
    ))
}

impl Lockstep {
    /// Make the chain of `compiled` with `reference` beside Heartwood, and walk it from its
    /// genesis block.
    pub fn prepare(compiled: &Compiled, reference: &mut dyn Reference) -> Result<Lockstep> {
        let chain = Chain::new(&compiled.scenario)?;
        let (recipe, rounds) = recipe(compiled, &chain)?;
        let mut queue: HashMap<u32, VecDeque<usize>> = HashMap::new();
        for (index, tx) in compiled.transactions.iter().enumerate() {
            queue.entry(tx.height).or_default().push_back(index);
        }
        let mut recording = Recording {
            inner: reference,
            compiled: &compiled.transactions,
            queue,
            verdicts: HashMap::new(),
            events: Vec::new(),
            genesis: Value::Null,
        };
        let scratch = ScratchDir::new("scenario-lockstep")?;
        make_with(
            &mut recording,
            &compiled.scenario.genesis_seed,
            &chain.profile,
            rounds,
            &recipe,
            scratch.path(),
        )
        .map_err(|e| match e {
            LockstepError::Recipe(m) => usage(m),
            other => Error::Oracle(format!("lockstep: {other}")),
        })?;
        let genesis = &recording.genesis;
        let held = Held {
            height: genesis["h"]
                .as_u64()
                .and_then(|h| u32::try_from(h).ok())
                .unwrap_or(1),
            id: genesis["id"].as_str().unwrap_or_default().to_owned(),
            transactions: Vec::new(),
        };
        Ok(Lockstep {
            lines: vec![genesis.clone()],
            chain: vec![held],
            events: recording.events,
            verdicts: recording.verdicts,
            position: 0,
        })
    }

    fn tip_held(&self) -> &Held {
        self.chain
            .last()
            .unwrap_or_else(|| unreachable!("the genesis block stays"))
    }

    /// Take the next event when it is a revert.
    fn take_revert(&mut self, action: &'static str) -> std::result::Result<u32, BackendError> {
        match self.events.get(self.position).cloned() {
            Some(Event::Reverted { lines }) => {
                self.position += 1;
                let count = lines.len();
                self.chain.truncate(self.chain.len().saturating_sub(count));
                self.lines.extend(lines);
                Ok(u32::try_from(count).unwrap_or(u32::MAX))
            }
            _ => Err(BackendError::Failed {
                action,
                message: format!(
                    "the chain made for the scenario does not revert after height {}",
                    self.tip_held().height
                ),
            }),
        }
    }
}

impl Backend for Lockstep {
    fn name(&self) -> &'static str {
        "lockstep"
    }

    fn nodes(&self) -> u32 {
        1
    }

    fn start(&mut self, _: u32, _: Part) -> std::result::Result<(), BackendError> {
        Err(unsupported("start a node"))
    }

    fn stop(&mut self, _: u32, _: Part, _: bool) -> std::result::Result<(), BackendError> {
        Err(unsupported("stop a node"))
    }

    fn restart(
        &mut self,
        _: u32,
        _: Part,
        _: RestartModes,
    ) -> std::result::Result<(), BackendError> {
        Err(unsupported("restart a node"))
    }

    fn partition(&mut self, _: &[Vec<u32>]) -> std::result::Result<(), BackendError> {
        Err(unsupported("partition"))
    }

    fn heal(&mut self) -> std::result::Result<(), BackendError> {
        Err(unsupported("heal"))
    }

    fn api_url(&self, _: u32) -> std::result::Result<String, BackendError> {
        Err(unsupported("call an API"))
    }

    fn archive(&mut self, _: &Path) -> std::result::Result<(), BackendError> {
        Err(unsupported("archive"))
    }

    fn submit(
        &mut self,
        node: u32,
        _: Via,
        transactions: &[CompiledTx],
    ) -> std::result::Result<Vec<Submitted>, BackendError> {
        self.tip(node)?;
        transactions
            .iter()
            .map(|t| {
                let (accepted, code, message) =
                    self.verdicts
                        .get(&t.id)
                        .cloned()
                        .ok_or_else(|| BackendError::Failed {
                            action: "submit",
                            message: format!(
                                "the chain made for the scenario never submitted {}",
                                t.reference
                            ),
                        })?;
                Ok(Submitted {
                    reference: t.reference.clone(),
                    accepted,
                    code,
                    message,
                })
            })
            .collect()
    }

    fn tip(&self, node: u32) -> std::result::Result<Tip, BackendError> {
        if node != 1 {
            return Err(BackendError::NoNode(node));
        }
        let held = self.tip_held();
        Ok(Tip {
            height: held.height,
            id: held.id.clone(),
        })
    }

    fn revert(&mut self, blocks: u32) -> std::result::Result<(), BackendError> {
        match self.take_revert("revert")? {
            taken if taken == blocks => Ok(()),
            taken => Err(BackendError::Failed {
                action: "revert",
                message: format!(
                    "the chain made for the scenario reverts {taken} blocks, not {blocks}"
                ),
            }),
        }
    }

    fn fork(&mut self, _: &str) -> std::result::Result<(), BackendError> {
        self.take_revert("fork").map(drop)
    }

    fn forged(&self, node: u32, id: &str) -> std::result::Result<bool, BackendError> {
        self.tip(node)?;
        Ok(self
            .chain
            .iter()
            .any(|b| b.transactions.iter().any(|t| t == id)))
    }

    fn wait(&mut self) -> std::result::Result<(), BackendError> {
        match self.events.get(self.position).cloned() {
            Some(Event::Applied {
                height,
                id,
                transactions,
                line,
            }) => {
                self.position += 1;
                self.chain.push(Held {
                    height,
                    id,
                    transactions,
                });
                self.lines.push(line);
                Ok(())
            }
            Some(Event::Reverted { .. }) => Err(BackendError::Failed {
                action: "wait for a block",
                message: format!(
                    "the chain made for the scenario reverts after height {}: a revert or fork step is due",
                    self.tip_held().height
                ),
            }),
            None => Err(BackendError::Failed {
                action: "wait for a block",
                message: format!(
                    "the chain made for the scenario ends at height {}",
                    self.tip_held().height
                ),
            }),
        }
    }

    fn verified(&self, name: &str) -> Option<std::result::Result<(), String>> {
        match name {
            "digest" | "singleTip" => Some(Ok(())),
            "revertSymmetry" => {
                let asymmetries = crate::digests::revert_asymmetries(&self.lines);
                Some(if asymmetries.is_empty() {
                    Ok(())
                } else {
                    Err(asymmetries.join("; "))
                })
            }
            _ => None,
        }
    }
}
