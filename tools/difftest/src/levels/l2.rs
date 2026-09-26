//! L2: a corpus chain replayed block by block in Heartwood, with a state digest line after every
//! step compared with the oracle's line of the same step.
//!
//! The replay decodes every block as a node receives it, checks the scheduled generator and the
//! reward before applying it, runs the supply check after every step, and follows the corpus's
//! revert plan. At the first difference it shows both sides' records of that step.
//!
//! The steps of a replay are [`actions`]: the genesis step, then for every block an apply, and
//! after a block that the revert plan names, its reverts (one line each) and the re-applies of
//! the reverted blocks. The oracle's judge (`replay-chain.js`) walks the same steps, so line `i`
//! of both files describes the same state.

use std::path::Path;

use heartwood_crypto::Block;
use heartwood_kernel::utils::calculate_forging_info::calculate_forging_info;
use heartwood_state::State;
use heartwood_state::digest::{self, DigestLine, Sections, Step};
use heartwood_state::round_state::MissedBlock;
use heartwood_state::supply::check_supply;
use heartwood_transactions::Handlers;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::{ANCHORS, Corpus, TS_DIGESTS};
use crate::digests::{self, sections_from_json};
use crate::errors::{Error, Result};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome, Report};

/// What an L2 run covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct L2Options {
    /// Also run the synthetic cases on top of the chain (needs the oracle).
    pub synthetic: bool,
}

/// One step of a replay after the genesis step; each gives one digest line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Apply the block at this height for the first time.
    Apply(u32),
    /// Revert the last block.
    Revert,
    /// Apply again the block at this height, which a revert took back.
    Reapply(u32),
}

/// The steps of a replay of `corpus` after the genesis step, in order.
pub fn actions(corpus: &Corpus) -> Vec<Action> {
    plan_actions(corpus.blocks.iter().map(|b| b.height), &corpus.revert_plan)
}

/// The steps for blocks at `heights`, each followed by the reverts and re-applies that `plan`
/// runs after it.
pub fn plan_actions(
    heights: impl IntoIterator<Item = u32>,
    plan: &crate::corpus::RevertPlan,
) -> Vec<Action> {
    let mut out = Vec::new();
    for height in heights {
        out.push(Action::Apply(height));
        for step in plan.after(height) {
            out.extend(std::iter::repeat_n(Action::Revert, step.blocks as usize));
            let first = height - step.blocks + 1;
            out.extend((first..=height).map(Action::Reapply));
        }
    }
    out
}

/// A problem of the Heartwood side that is not a digest difference: a refused block, a schedule
/// or reward mismatch, a failed supply check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The digest line the step writes (from 1: line 1 is the genesis step).
    pub line: usize,
    /// The block height of the step.
    pub height: u32,
    /// What went wrong.
    pub message: String,
}

/// Heartwood's replay of a corpus: its digest lines, and the problems met.
#[derive(Debug, Clone, Default)]
pub struct Replay {
    /// One line per step, the genesis step first.
    pub lines: Vec<DigestLine>,
    /// The problems, in step order.
    pub problems: Vec<Problem>,
    /// The records of the last step, when the replay was asked to stop at a line.
    pub records: Option<Sections>,
}

fn decode(corpus: &Corpus, height: u32) -> std::result::Result<Block, String> {
    let block = corpus
        .block(height)
        .ok_or_else(|| format!("the corpus has no block {height}"))?;
    let decoded =
        Block::receive(&block.bytes, &corpus.milestones).map_err(|e| format!("decode: {e}"))?;
    if decoded.id() != &block.id {
        return Err(format!(
            "decodes to id {}, the corpus says {}",
            decoded.id(),
            block.id
        ));
    }
    Ok(decoded)
}

/// Replay `corpus` in Heartwood, up to and including line `stop_at` (all lines when `None`),
/// keeping the records of that line.
pub fn replay(corpus: &Corpus, stop_at: Option<usize>) -> Replay {
    let milestones = &corpus.milestones;
    let handlers = Handlers;
    let mut out = Replay::default();
    let mut state = match State::from_genesis(&corpus.genesis, milestones, &handlers) {
        Ok(state) => state,
        Err(e) => {
            out.problems.push(Problem {
                line: 1,
                height: 1,
                message: format!("the genesis block was refused: {e}"),
            });
            return out;
        }
    };
    let mut chain = [0u8; 32];
    let mut push = |out: &mut Replay,
                    state: &State,
                    step: Step,
                    missed: &[MissedBlock],
                    reward: Option<u64>| {
        let line = digest::height_digest(state, milestones, step, missed, reward, &chain);
        chain = line.chain_digest;
        out.lines.push(line);
        if let Err(e) = check_supply(state) {
            out.problems.push(Problem {
                line: out.lines.len(),
                height: state.height(),
                message: format!("supply check: {e}"),
            });
        }
        if stop_at == Some(out.lines.len()) {
            out.records = Some(digest::records(state, milestones, missed));
        }
    };
    push(&mut out, &state, Step::Genesis, &[], None);
    for action in actions(corpus) {
        if stop_at.is_some_and(|stop| out.lines.len() >= stop) {
            break;
        }
        let line = out.lines.len() + 1;
        match action {
            Action::Revert => {
                if let Err(e) = state.revert_block(milestones, &handlers) {
                    out.problems.push(Problem {
                        line,
                        height: state.height(),
                        message: format!("revert refused: {e}"),
                    });
                    break;
                }
                push(&mut out, &state, Step::Revert, &[], None);
            }
            Action::Apply(height) | Action::Reapply(height) => {
                let step = if matches!(action, Action::Apply(_)) {
                    Step::Apply
                } else {
                    Step::Reapply
                };
                let block = match decode(corpus, height) {
                    Ok(block) => block,
                    Err(message) => {
                        out.problems.push(Problem {
                            line,
                            height,
                            message,
                        });
                        break;
                    }
                };
                let header = block.header();
                let generator = header.generator_public_key;
                let info = calculate_forging_info(i64::from(header.timestamp), height, milestones);
                let scheduled = usize::try_from(info.current_forger)
                    .ok()
                    .and_then(|i| state.forging_delegates().get(i));
                if scheduled.map(|d| d.public_key) != Some(generator) {
                    out.problems.push(Problem {
                        line,
                        height,
                        message: format!(
                            "the schedule names {} for the slot, the block's generator is {}",
                            scheduled.map_or_else(|| "nobody".into(), |d| d.username.clone()),
                            generator.to_hex()
                        ),
                    });
                }
                let reward = match state.reward_for_block_in_round(height, &generator, milestones) {
                    Ok(r) => {
                        if r.reward != header.reward {
                            out.problems.push(Problem {
                                line,
                                height,
                                message: format!(
                                    "the reward in its round is {}, the header says {}",
                                    r.reward, header.reward
                                ),
                            });
                        }
                        Some(r.reward)
                    }
                    Err(e) => {
                        out.problems.push(Problem {
                            line,
                            height,
                            message: format!("no reward for the generator: {e}"),
                        });
                        None
                    }
                };
                match state.apply_block(&block, milestones, &handlers) {
                    Ok(applied) => push(&mut out, &state, step, &applied.missed, reward),
                    Err(e) => {
                        out.problems.push(Problem {
                            line,
                            height,
                            message: format!("refused: {e}"),
                        });
                        break;
                    }
                }
            }
        }
    }
    out
}

/// The judge's anchor results of a corpus, and whether they hold.
pub fn judge_report(dir: &Path) -> Result<(bool, Value)> {
    let path = dir.join(ANCHORS);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let value: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
        path: path.clone(),
        message: e.to_string(),
    })?;
    Ok((value.get("ok") == Some(&Value::Bool(true)), value))
}

fn describe_judge(judge: &Value) -> String {
    let short = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(|s| s.chars().take(12).collect::<String>())
            .unwrap_or_else(|| "unknown".into())
    };
    format!(
        "judge: reference commit {} (tree {}), oracle {} {}, {} lines",
        short(judge.pointer("/reference/commit")),
        short(judge.pointer("/reference/tree")),
        judge
            .pointer("/oracle/mode")
            .and_then(Value::as_str)
            .unwrap_or("?"),
        short(judge.pointer("/oracle/image")),
        judge.get("lines").and_then(Value::as_u64).unwrap_or(0)
    )
}

fn step_label(line: &Value) -> String {
    format!(
        "{} {}",
        line.get("step").and_then(Value::as_str).unwrap_or("?"),
        line.get("h").and_then(Value::as_u64).unwrap_or(0)
    )
}

/// The oracle's records at digest line `line`: a session of the oracle walks the corpus's steps
/// up to that line and dumps them.
pub fn oracle_records(server: &mut OracleServer, corpus: &Corpus, line: usize) -> Result<Sections> {
    let crypto = corpus_crypto(corpus)?;
    let open = call(server, "chain.open", 1, &json!({"crypto": crypto}))?;
    let session = open.get("session").cloned().unwrap_or(Value::Null);
    for (done, action) in (1..).zip(actions(corpus)) {
        if done >= line {
            break;
        }
        match action {
            Action::Revert => {
                call(
                    server,
                    "chain.revert",
                    1,
                    &json!({"session": session, "count": 1}),
                )?;
            }
            Action::Apply(h) | Action::Reapply(h) => {
                let bytes = corpus
                    .block(h)
                    .map(|b| heartwood_crypto::utils::hex::encode(&b.bytes));
                let step = if matches!(action, Action::Apply(_)) {
                    "apply"
                } else {
                    "reapply"
                };
                call(
                    server,
                    "chain.apply",
                    1,
                    &json!({"session": session, "hex": bytes, "step": step}),
                )?;
            }
        }
    }
    let dump = call(server, "chain.dump", 1, &json!({"session": session}))?;
    call(server, "chain.close", 1, &json!({"session": session}))?;
    dump.get("sections")
        .and_then(sections_from_json)
        .ok_or_else(|| Error::Oracle("chain.dump sent no sections".into()))
}

/// The chain definition files of a corpus, as `chain.open` takes them.
pub fn corpus_crypto(corpus: &Corpus) -> Result<Value> {
    let mut crypto = serde_json::Map::new();
    for name in crate::corpus::CRYPTO_FILES {
        let path = corpus.dir.join("crypto").join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let value: Value = serde_json::from_str(&text).map_err(|e| Error::Corpus {
            path: path.clone(),
            message: e.to_string(),
        })?;
        crypto.insert(name.to_owned(), value);
    }
    Ok(Value::Object(crypto))
}

/// One oracle call that must succeed.
pub fn call(server: &mut OracleServer, op: &str, height: u64, args: &Value) -> Result<Value> {
    server
        .call(op, "devnet", height, args)?
        .map_err(|e| Error::Oracle(format!("{op} failed: {}: {}", e.class, e.message)))
}

/// Replay `corpus` and compare every step with its recorded oracle digests. With `oracle`, the
/// first differing step is dumped on both sides and diffed by section, wallet and field, and the
/// synthetic cases run when `options` asks for them.
pub fn run(
    corpus: &Corpus,
    options: &L2Options,
    mut oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let (judge_ok, judge) = judge_report(&corpus.dir)?;
    let oracle_lines = digests::read(&corpus.dir.join(TS_DIGESTS))?;
    let file = format!("{}/{TS_DIGESTS}", corpus.name());
    let mut results = Vec::new();
    let case = |line: usize, op: &str, label: String, outcome: Outcome| CaseResult {
        file: file.clone(),
        line,
        op: op.to_owned(),
        label,
        height: 0,
        outcome,
    };
    if !judge_ok {
        results.push(case(
            0,
            "judge",
            "anchors".into(),
            Outcome::HarnessError(
                "the oracle's replay failed its anchors: it is not a judge (see anchors.json)"
                    .into(),
            ),
        ));
    }
    let heartwood = replay(corpus, None);
    for problem in &heartwood.problems {
        results.push(CaseResult {
            height: u64::from(problem.height),
            ..case(
                problem.line,
                "check",
                format!("height {}", problem.height),
                Outcome::HarnessError(problem.message.clone()),
            )
        });
    }
    let mut first_mismatch = None;
    for i in 0..oracle_lines.len().max(heartwood.lines.len()) {
        let theirs = oracle_lines.get(i).cloned().unwrap_or(Value::Null);
        let ours = heartwood
            .lines
            .get(i)
            .map_or(Value::Null, digests::line_value);
        let label = step_label(if theirs.is_null() { &ours } else { &theirs });
        let differences = compare::diff(&theirs, &ours);
        let outcome = if differences.is_empty() {
            Outcome::Pass
        } else {
            first_mismatch.get_or_insert(i + 1);
            Outcome::Mismatch(differences)
        };
        let height = theirs.get("h").and_then(Value::as_u64).unwrap_or(0);
        let op = theirs
            .get("step")
            .and_then(Value::as_str)
            .unwrap_or("line")
            .to_owned();
        results.push(CaseResult {
            height,
            ..case(i + 1, &op, label, outcome)
        });
    }
    let ours: Vec<Value> = heartwood.lines.iter().map(digests::line_value).collect();
    for message in digests::revert_asymmetries(&ours) {
        results.push(case(
            0,
            "revert",
            "symmetry".into(),
            Outcome::HarnessError(message),
        ));
    }
    if let (Some(line), Some(server)) = (first_mismatch, oracle.as_deref_mut()) {
        let theirs = oracle_records(server, corpus, line)?;
        let ours = replay(corpus, Some(line)).records;
        let message = match ours {
            Some(ours) => digests::section_differences(&theirs, &ours).join("\n"),
            None => "Heartwood's replay stopped before this step".into(),
        };
        results.push(case(
            line,
            "dump",
            format!("records at line {line}"),
            Outcome::HarnessError(message),
        ));
    }
    if options.synthetic {
        match oracle {
            Some(server) => results.extend(crate::synthetic::run(corpus, server)?),
            None => return Err(Error::Usage("the synthetic cases need the oracle".into())),
        }
    }
    Ok(Report {
        title: super::Level::L2.title().to_string(),
        source: describe_judge(&judge),
        files: 1,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{RevertPlan, RevertStep};

    #[test]
    fn actions_follow_the_revert_plan() {
        let plan = RevertPlan {
            seed: 0,
            round_length: 53,
            tip: 5,
            steps: vec![
                RevertStep {
                    after_height: 3,
                    blocks: 2,
                },
                RevertStep {
                    after_height: 3,
                    blocks: 1,
                },
            ],
        };
        use Action::*;
        assert_eq!(
            plan_actions([2, 3, 4], &plan),
            vec![
                Apply(2),
                Apply(3),
                Revert,
                Revert,
                Reapply(2),
                Reapply(3),
                Revert,
                Reapply(3),
                Apply(4)
            ]
        );
    }
}
