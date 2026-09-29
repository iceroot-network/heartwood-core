//! The recovery commands in a mixed run (node 6 of runs `w` and `w0`): the pooled transfer kept
//! across a restart and gone after `pool clear`, the replay start after `state clear` and the vote
//! it admits, and the rollback's store summaries against the oracle's prediction for the run's own
//! chain, before and after the start that follows it, then the start's log lines and the re-sync.
//!
//! What the checks read of the recipe's records (the reading contract):
//!
//! - `steps.jsonl`: `summary-before`, `summary-rolled-back` and `summary-restarted`, around the
//!   step `rollback`; each summary is the step's `summary` object (in the record or its `detail`)
//!   or else the file `summaries/<before|rolled-back|restarted>-node6.json`, with the store's `tip`
//!   (a height, or an object with its `height`), highest `round` (or `highestRound`), `missed` rows
//!   (`missedRows`, `missedSlots`) and `transactions` (`transactionCount`). The transactions are
//!   those of every stored block, the genesis block's included. A summary file whose `ok` is false
//!   is no summary. The step `rollback` may hold the command's `height` (133 when it does not);
//!   where it lands is derived from that height.
//! - `r1`, `r2`, `r3` and `rollback-open` (the isolation lifted and the re-sync) must be `ok`; a
//!   flag they name (`admitted`, `pooled`, `keptAcrossRestart`; `goneAfterClear`; `replayPath`)
//!   must not be `false`.
//! - `r3-vote`: `admitted`, or its `outcome` (`accepted`, or `rejected`), and when it was submitted
//!   (`submittedAt`, else the step's start).
//! - The oracle's prediction of the rollback is read from `predictions/rollback-<cut>-<height>.json`
//!   in the run's directory when the file is there (the oracle's `chain.rollback` result: `target`
//!   and `summary`), and asked of the oracle otherwise.
//! - Node 6's own log: `logs/node6` when the node is a reference node.

use std::path::Path;

use heartwood_crypto::Block;
use serde_json::{Value, json};

use super::fresh;
use super::mixed::{self, guard};
use super::nodes::{self, rows_of};
use super::run::{self, Step};
use super::{Cases, MixedRun};
use crate::corpus::{self, Corpus};
use crate::errors::{Error, Result};
use crate::levels::l2;
use crate::store::{self, ScratchDir};

const SECTION: &str = "recovery";

/// The store's summary after a rollback or at a cut: the tip, the highest round with rows, and
/// the numbers of missed-slot rows and of transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Summary {
    /// The tip's height.
    pub tip: u64,
    /// The highest round with rows.
    pub round: u64,
    /// The missed-slot rows.
    pub missed: u64,
    /// The stored transactions.
    pub transactions: u64,
}

impl Summary {
    /// The summary an object holds, under the keys the recipe and the oracle use.
    fn of(v: &Value) -> Option<Summary> {
        let number = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k).and_then(Value::as_u64));
        let tip = v
            .get("tip")
            .and_then(|t| t.as_u64().or_else(|| t.get("height")?.as_u64()))
            .or_else(|| number(&["height"]));
        Some(Summary {
            tip: tip?,
            round: number(&["round", "highestRound"])?,
            missed: number(&["missed", "missedRows", "missedSlots"])?,
            transactions: number(&["transactions", "transactionCount"])?,
        })
    }

    /// The summary a step recorded.
    fn of_step(step: &Step) -> Option<Summary> {
        [
            step.record.get("summary"),
            step.record.pointer("/detail/summary"),
            step.record.get("detail"),
            Some(&step.record),
        ]
        .into_iter()
        .flatten()
        .find_map(Summary::of)
    }

    /// The summary as text.
    fn describe(&self) -> String {
        format!(
            "tip {}, highest round {}, {} missed rows, {} transactions",
            self.tip, self.round, self.missed, self.transactions
        )
    }
}

/// How two counts of missed rows compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Missed {
    /// Equal.
    Equal,
    /// The oracle's count is above node 6's by exactly the rows the reference backfills at height
    /// 2 when the first block is not in slot 1, which Heartwood does not write.
    Backfill(u64),
    /// Anything else.
    Differ,
}

/// The rows a chain's first block leaves behind it in the reference's backfill: one per slot
/// before the block's own, the slot 0 being the genesis block's.
pub(super) fn backfilled(corpus: &Corpus, block_time: u32) -> u64 {
    let Some(block) = corpus.blocks.iter().find(|b| b.height == 2) else {
        return 0;
    };
    let Ok(decoded) = Block::decode(&block.bytes, &corpus.milestones) else {
        return 0;
    };
    (u64::from(decoded.header().timestamp) / u64::from(block_time.max(1))).saturating_sub(1)
}

/// The missed-row count of the oracle (`oracle`) against node 6's (`ours`), given the rows the
/// backfill of height 2 adds.
pub(super) fn compare_missed(oracle: u64, ours: u64, backfill: u64) -> Missed {
    if oracle == ours {
        Missed::Equal
    } else if backfill > 0 && oracle == ours + backfill {
        Missed::Backfill(backfill)
    } else {
        Missed::Differ
    }
}

/// The summary of the corpus cut at `cut`, from its blocks and its reference rows: the tip, the
/// highest round row, the missed rows at or below the tip and the transactions of every block
/// from the genesis block up.
fn cut_summary(corpus: &Corpus, dir: &Path, cut: u32, round_length: u32) -> Result<Summary> {
    let mut transactions = u64::from(corpus.genesis.header().number_of_transactions);
    for block in corpus.blocks.iter().filter(|b| b.height <= cut) {
        let decoded = Block::decode(&block.bytes, &corpus.milestones)
            .map_err(|e| Error::Usage(format!("block {} does not decode: {e}", block.height)))?;
        transactions += u64::from(decoded.header().number_of_transactions);
    }
    let missed = rows_of(dir)?
        .missed
        .iter()
        .filter(|m| m.1 <= u64::from(cut))
        .count();
    Ok(Summary {
        tip: u64::from(cut),
        round: store::last_round(cut, round_length),
        missed: missed as u64,
        transactions,
    })
}

/// The oracle's `chain.rollback` for a cut of the run's chain: read from the run's predictions
/// when the file is there, else asked of the oracle.
struct Prediction {
    /// The height the rollback lands on.
    target: u64,
    /// The store after it.
    summary: Summary,
}

/// The prediction of the rollback `asked` over the corpus in `dir` cut at `cut`.
fn prediction(run: &MixedRun<'_>, dir: &Path, cut: u32, asked: u64) -> Result<Prediction> {
    let cache = run
        .run_dir
        .join("predictions")
        .join(format!("rollback-{cut}-{asked}.json"));
    let result = if cache.exists() {
        run::read_json(&cache)?
    } else {
        let oracle = (run.oracle)()?;
        let mut server = oracle.server()?;
        let opened = l2::call(
            &mut server,
            "chain.open",
            1,
            &json!({"corpus": dir.display().to_string(), "height": cut}),
        )?;
        let session = opened.get("session").cloned().unwrap_or(Value::Null);
        l2::call(
            &mut server,
            "chain.rollback",
            1,
            &json!({"session": session, "height": asked}),
        )?
    };
    if let Some(lines) = result.get("refused") {
        return Err(Error::Oracle(format!(
            "the oracle refused the rollback to {asked}: {lines}"
        )));
    }
    let summary = result.get("summary").and_then(Summary::of).ok_or_else(|| {
        Error::Oracle(format!(
            "the rollback's prediction has no summary: {result}"
        ))
    })?;
    let target = result
        .get("target")
        .and_then(Value::as_u64)
        .unwrap_or(summary.tip);
    Ok(Prediction { target, summary })
}

/// Whether node 6 is a Heartwood relay (run `w`; in a dry run of the recipe it is a reference
/// relay, which the checks judge as run `w0`'s).
fn heartwood_node6(run: &MixedRun<'_>) -> bool {
    mixed::heartwood(run.manifest)
        .iter()
        .any(|n| n.name == "node6")
}

/// A flag a step recorded, in its record or its `detail`.
fn flag(step: &Step, key: &str) -> Option<bool> {
    step.record
        .get(key)
        .or_else(|| step.record.pointer(&format!("/detail/{key}")))
        .and_then(Value::as_bool)
}

/// The recovery commands' checks.
pub(super) fn check(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    if !matches!(run.manifest.run.as_str(), "w" | "w0") {
        cases.record(SECTION, "this run has no recovery commands on node 6");
        return Ok(());
    }
    let steps = match run::steps(run.run_dir) {
        Ok(s) => s,
        Err(e) => {
            cases.fail(SECTION, "the recovery steps", e.to_string());
            return Ok(());
        }
    };
    guard(
        cases,
        SECTION,
        "the pooled transfer and the clears",
        |cases| {
            transfer(&steps, cases);
            Ok(())
        },
    )?;
    guard(cases, SECTION, "the rollback's store summaries", |cases| {
        summaries(run, &steps, cases)
    })?;
    guard(cases, SECTION, "the vote after the state clear", |cases| {
        vote(run, &steps, cases)
    })?;
    guard(
        cases,
        SECTION,
        "node6: the start after the rollback",
        |cases| start_lines(run, &steps, cases),
    )?;
    guard(cases, SECTION, "node6: re-synced", |cases| {
        resync(run, &steps, cases)
    })
}

// ---------------------------------------------------------------------------------------------
// The pooled transfer and the clears
// ---------------------------------------------------------------------------------------------

/// r1 (the transfer admitted and pooled, kept across a plain restart), r2 (gone after `pool
/// clear`) and r3 (the `state clear` start took the replay path).
fn transfer(steps: &[Step], cases: &mut Cases) {
    let wanted: [(&str, &str, &[&str]); 3] = [
        (
            "r1",
            "the transfer was admitted, pooled and kept across a plain restart",
            &["admitted", "pooled", "keptAcrossRestart"],
        ),
        (
            "r2",
            "the transfer was gone after pool clear",
            &["goneAfterClear"],
        ),
        (
            "r3",
            "the state clear start took the replay path",
            &["replayPath"],
        ),
    ];
    for (name, what, flags) in wanted {
        let label = format!("node6: {what}");
        let Some(step) = mixed::step(steps, name) else {
            cases.fail(SECTION, label, format!("no {name} step"));
            continue;
        };
        let refuted: Vec<&str> = flags
            .iter()
            .copied()
            .filter(|k| flag(step, k) == Some(false))
            .collect();
        if !step.ok {
            cases.fail(
                SECTION,
                label,
                format!("the {name} step did not meet its expectation"),
            );
        } else if !refuted.is_empty() {
            cases.fail(SECTION, label, format!("{} was false", refuted.join(", ")));
        } else {
            cases.pass(SECTION, format!("{label} ({name})"));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The store summaries
// ---------------------------------------------------------------------------------------------

/// The summary step `name` recorded, in its record or in the file `summaries/<phase>-node6.json`,
/// or a failed case.
fn recorded(
    run: &MixedRun<'_>,
    steps: &[Step],
    (name, phase): (&str, &str),
    cases: &mut Cases,
    label: &str,
) -> Option<Summary> {
    let Some(step) = mixed::step(steps, name) else {
        cases.fail(SECTION, label, format!("no {name} step"));
        return None;
    };
    if !step.ok {
        cases.fail(
            SECTION,
            label,
            format!("the {name} step did not meet its expectation"),
        );
        return None;
    }
    if let Some(summary) = Summary::of_step(step) {
        return Some(summary);
    }
    let path = summary_file(run, phase);
    let file = match run::read_json(&path) {
        Ok(v) => v,
        Err(e) => {
            cases.fail(
                SECTION,
                label,
                format!("the {name} step records no store summary, and {e}"),
            );
            return None;
        }
    };
    let summary = (file.get("ok") != Some(&Value::Bool(false)))
        .then(|| Summary::of(&file))
        .flatten();
    if summary.is_none() {
        cases.fail(
            SECTION,
            label,
            format!(
                "{} holds no store summary (tip, round, missed, transactions): {}",
                path.display(),
                file.get("note").and_then(Value::as_str).unwrap_or("")
            ),
        );
    }
    summary
}

/// The file the recipe writes node 6's summary of `phase` into.
fn summary_file(run: &MixedRun<'_>, phase: &str) -> std::path::PathBuf {
    run.run_dir
        .join("summaries")
        .join(format!("{phase}-node6.json"))
}

/// The summary steps and their phases: before the rollback, after it and after the start.
const BEFORE: (&str, &str) = ("summary-before", "before");
const ROLLED_BACK: (&str, &str) = ("summary-rolled-back", "rolled-back");
const RESTARTED: (&str, &str) = ("summary-restarted", "restarted");

/// One summary against another: the fields that differ, as a problem list; the missed rows by the
/// backfill caveat, which a run records instead of failing.
fn differences(
    cases: &mut Cases,
    label: &str,
    theirs: &Summary,
    ours: &Summary,
    backfill: u64,
    theirs_name: &str,
) -> bool {
    let mut problems = Vec::new();
    if theirs.tip != ours.tip {
        problems.push(format!(
            "tip {} against {theirs_name}'s {}",
            ours.tip, theirs.tip
        ));
    }
    if theirs.round != ours.round {
        problems.push(format!(
            "highest round row {} against {theirs_name}'s {}",
            ours.round, theirs.round
        ));
    }
    if theirs.transactions != ours.transactions {
        problems.push(format!(
            "{} transactions against {theirs_name}'s {}",
            ours.transactions, theirs.transactions
        ));
    }
    match compare_missed(theirs.missed, ours.missed, backfill) {
        Missed::Equal => {}
        Missed::Backfill(n) => cases.record(
            SECTION,
            format!(
                "{label}: {theirs_name} counts {n} more missed rows than node 6, the rows the reference writes for the slots before block 2, which Heartwood does not"
            ),
        ),
        Missed::Differ => problems.push(format!(
            "{} missed rows against {theirs_name}'s {}",
            ours.missed, theirs.missed
        )),
    }
    if problems.is_empty() {
        true
    } else {
        cases.fail(SECTION, label, problems.join("; "));
        false
    }
}

/// The three summaries of node 6: before the command (equal to the corpus cut at its tip), after
/// the rollback (equal to the oracle's prediction) and after the start that follows.
fn summaries(run: &MixedRun<'_>, steps: &[Step], cases: &mut Cases) -> Result<()> {
    let length = run.manifest.round_length;
    let before = recorded(
        run,
        steps,
        BEFORE,
        cases,
        "node6: the summary before the rollback",
    );
    let after = recorded(
        run,
        steps,
        ROLLED_BACK,
        cases,
        "node6: the summary after the rollback",
    );
    let started = recorded(
        run,
        steps,
        RESTARTED,
        cases,
        "node6: the summary after the start",
    );
    let (Some(before), Some(after), Some(started)) = (before, after, started) else {
        return Ok(());
    };
    let chains = mixed::chains(run)?;
    let last = chains.last();
    let cut = u32::try_from(before.tip).unwrap_or(u32::MAX);
    if cut > last.tip {
        cases.fail(
            SECTION,
            "node6: the summary before the rollback",
            format!("its tip {cut} is above the final chain's {}", last.tip),
        );
        return Ok(());
    }
    let corpus = Corpus::load(&last.dir)?;
    let backfill = backfilled(&corpus, run.manifest.block_time);
    // The corpus cut at node 6's tip.
    let cut_summary = cut_summary(&corpus, &last.dir, cut, length)?;
    let label = "node6: the summary before the rollback equals the corpus cut at its tip";
    // Neither side has started since the chain began: no backfilled row on either.
    if differences(cases, label, &cut_summary, &before, 0, "the cut") {
        cases.pass(SECTION, format!("{label} ({})", before.describe()));
    }
    // The oracle's rollback over the cut: node 6's summary after the command.
    let asked = mixed::rollback_asked(steps);
    let label =
        format!("node6: the summary after the rollback to {asked} equals the oracle's prediction");
    let predicted = match prediction(run, &last.dir, cut, asked) {
        Ok(p) => p,
        Err(e) if e.to_string().contains("is not built") => {
            cases.not_built(SECTION, &label);
            return Ok(());
        }
        Err(e) => {
            cases.fail(SECTION, label, e.to_string());
            return Ok(());
        }
    };
    if predicted.target != after.tip {
        cases.fail(
            SECTION,
            &label,
            format!(
                "node 6's tip is {}, the rollback lands on {}",
                after.tip, predicted.target
            ),
        );
    } else if differences(cases, &label, &predicted.summary, &after, 0, "the oracle") {
        cases.pass(SECTION, format!("{label} ({})", after.describe()));
    }
    // The start after it, as the reference's start after that rollback: the same tip and
    // transactions, the next round's rows saved, and, when the rollback left no missed row, the
    // productivity backfill's rows for the slots before block 2 (none when block 2 is in slot 1).
    // The reference (run w0) must hold them; Heartwood (run w) does not write them, which is
    // recorded; any other difference fails.
    let label =
        "node6: the summary after the start equals the oracle's restart after that rollback";
    let adds = if predicted.summary.missed == 0 {
        backfill
    } else {
        0
    };
    let mut wanted = predicted.summary;
    wanted.round = predicted.target / u64::from(length.max(1)) + 1;
    wanted.missed += adds;
    let allowed = if heartwood_node6(run) { adds } else { 0 };
    if started.round != wanted.round {
        cases.fail(
            SECTION,
            label,
            format!(
                "the start did not save round {}'s rows: the highest round row is {}",
                wanted.round, started.round
            ),
        );
    } else if differences(cases, label, &wanted, &started, allowed, "the oracle") {
        cases.pass(SECTION, format!("{label} ({})", started.describe()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The vote
// ---------------------------------------------------------------------------------------------

/// The lines of node 6's log, when it is a reference node with a log directory.
fn node6_log(run: &MixedRun<'_>) -> Option<Vec<nodes::LogLine>> {
    nodes::read_logs(&run.run_dir.join("logs").join("node6")).ok()
}

/// The vote for the delegate node 6 had recorded as attested: admitted by a Heartwood node 6 (run
/// w); by a reference node 6 (run w0) refused when it was submitted before node 6's first contact
/// with node 3 after r3's start (`contactNode3At` of the vote's or r3's record, else node 6's first
/// "Accepted new peer" line for node 3), and otherwise not shown live.
fn vote(run: &MixedRun<'_>, steps: &[Step], cases: &mut Cases) -> Result<()> {
    let label = "node6: the vote after the state clear";
    let Some(step) = mixed::step(steps, "r3-vote") else {
        cases.fail(SECTION, label, "no r3-vote step");
        return Ok(());
    };
    let admitted = flag(step, "admitted").or_else(|| match step.text("outcome").as_deref() {
        Some("accepted") => Some(true),
        Some("rejected" | "refused") => Some(false),
        _ => None,
    });
    if heartwood_node6(run) {
        if step.ok && admitted != Some(false) {
            cases.pass(SECTION, format!("{label} was admitted"));
        } else {
            cases.fail(SECTION, label, "the vote was refused or its step failed");
        }
        return Ok(());
    }
    let submitted = step.time("submittedAt").or(step.start);
    let node3 = mixed::ip_of(run.manifest, "node3").unwrap_or("").to_owned();
    let r3 = mixed::step(steps, "r3");
    let after = r3.and_then(|s| s.start).unwrap_or(i64::MIN);
    // The first contact: the time the recipe recorded, else node 6's first acceptance of node 3
    // after r3's start. A failed attempt to reach node 3 (an error line naming it) is none.
    let accepted = format!("Accepted new peer {node3}:");
    let logged = node6_log(run).and_then(|lines| {
        lines
            .iter()
            .filter(|l| l.at.is_some_and(|t| t >= after) && l.text.starts_with(&accepted))
            .filter_map(|l| l.at)
            .min()
    });
    let contact = [step, r3.unwrap_or(step)]
        .iter()
        .filter_map(|s| s.time("contactNode3At"))
        .chain(logged)
        .min();
    match (submitted, contact) {
        (Some(s), Some(c)) if s >= c => cases.record(
            SECTION,
            format!(
                "{label} not shown live: node 6 had contacted node3 before the vote was submitted"
            ),
        ),
        (Some(_), _) | (None, None) => {
            if admitted == Some(false) {
                cases.pass(
                    SECTION,
                    format!("{label} was refused before node 6's contact with node3, as the reference does"),
                );
            } else {
                cases.fail(
                    SECTION,
                    label,
                    "the vote was submitted before node 6's contact with node3 and was not refused",
                );
            }
        }
        (None, Some(_)) => cases.fail(
            SECTION,
            label,
            "the vote's submission time was not recorded",
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The start's lines
// ---------------------------------------------------------------------------------------------

/// A marker of the start after a rollback: the text a line must hold.
struct Marker {
    /// A name for the messages.
    name: &'static str,
    /// The text.
    text: String,
}

/// Run w0's node 6 started at the target with the reference's lines in their order: the tip's line,
/// the loader's refusal of the saved states above the target, the integrity check, the rebuild,
/// the productivity calculation only when no missed row was left, and the round's start and save.
fn start_lines(run: &MixedRun<'_>, steps: &[Step], cases: &mut Cases) -> Result<()> {
    let label = "node6: the lines of the start after the rollback";
    if heartwood_node6(run) {
        cases.record(
            SECTION,
            "the lines of the start after the rollback are the reference's: a reference node 6 shows them, and a Heartwood node 6 is judged by its digest lines",
        );
        return Ok(());
    }
    let dir = run.run_dir.join("logs").join("node6");
    let lines = nodes::read_logs(&dir)?;
    let target = u64::from(mixed::rollback_target(steps, run.manifest.round_length));
    let round = target / u64::from(run.manifest.round_length.max(1)) + 1;
    let since = mixed::step(steps, "rollback")
        .and_then(|s| s.start)
        .unwrap_or(i64::MIN);
    let left = mixed::step(steps, ROLLED_BACK.0)
        .and_then(Summary::of_step)
        .or_else(|| {
            run::read_json(&summary_file(run, ROLLED_BACK.1))
                .ok()
                .as_ref()
                .and_then(Summary::of)
        })
        .map_or(0, |s| s.missed);
    let tip_line = format!("Last block in database: {target}");
    let Some(begin) = lines
        .iter()
        .position(|l| l.at.is_some_and(|t| t >= since) && l.text == tip_line)
    else {
        cases.fail(
            SECTION,
            label,
            format!("no line \"{tip_line}\" after the rollback command"),
        );
        return Ok(());
    };
    let mut wanted = vec![
        Marker {
            name: "the loader's fresh state",
            text: "fresh state will now be generated".into(),
        },
        Marker {
            name: "the integrity check",
            text: "Verifying database integrity".into(),
        },
        Marker {
            name: "the integrity check's end",
            text: "Verified database integrity".into(),
        },
        Marker {
            name: "the delegate count",
            text: "Number of registered delegates".into(),
        },
    ];
    if left == 0 {
        wanted.push(Marker {
            name: "the productivity calculation",
            text: "Calculating productivity data".into(),
        });
    }
    wanted.push(Marker {
        name: "the round's start",
        text: format!("Starting Round {round}"),
    });
    wanted.push(Marker {
        name: "the round's save",
        text: format!("Saving round {round}"),
    });
    let mut at = begin + 1;
    let mut problems = Vec::new();
    for marker in &wanted {
        match lines[at..]
            .iter()
            .position(|l| l.text.contains(&marker.text))
        {
            Some(i) => at += i + 1,
            None => {
                problems.push(format!(
                    "no line for {} (\"{}\") after the previous one",
                    marker.name, marker.text
                ));
                break;
            }
        }
    }
    // The saved states the loader refused were all above the target.
    let refused = lines[begin..at.min(lines.len())]
        .iter()
        .filter_map(|l| {
            l.text
                .strip_prefix("The block from the saved state at height ")
                .and_then(|r| r.split_whitespace().next())
                .and_then(|n| n.parse::<u64>().ok())
        })
        .filter(|h| *h <= target)
        .count();
    if refused > 0 {
        problems.push(format!(
            "the loader refused {refused} saved states at or below the target {target}"
        ));
    }
    if left > 0
        && lines[begin..at.min(lines.len())]
            .iter()
            .any(|l| l.text.contains("Calculating productivity data"))
    {
        problems.push(format!(
            "\"Calculating productivity data\" was logged with {left} missed rows left"
        ));
    }
    if problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "{label}: from \"{tip_line}\" to \"Saving round {round}\" in the reference's order"
            ),
        );
    } else {
        for p in problems {
            cases.fail(SECTION, label, p);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The re-sync
// ---------------------------------------------------------------------------------------------

/// Node 6 re-synced to the tip: its step, its last block the final chain's, and (a reference
/// node) its round rows equal to node 1's for every round.
fn resync(run: &MixedRun<'_>, steps: &[Step], cases: &mut Cases) -> Result<()> {
    let label = "node6: re-synced to the tip";
    let Some(step) = mixed::step(steps, "rollback-open") else {
        cases.fail(SECTION, label, "no rollback-open step");
        return Ok(());
    };
    if !step.ok || step.text("synced").is_some_and(|s| s != "yes") {
        cases.fail(
            SECTION,
            label,
            "the rollback-open step did not meet its expectation",
        );
        return Ok(());
    }
    let chains = mixed::chains(run)?;
    let last = chains.last();
    let corpus = Corpus::load(&last.dir)?;
    let want = corpus
        .block_id(last.tip)
        .map(|i| i.to_hex())
        .unwrap_or_default();
    let final_json = run::read_json(&run.run_dir.join("final.json"))?;
    let tip = final_json.pointer("/tips/node6/id").and_then(Value::as_str);
    if tip == Some(want.as_str()) {
        cases.pass(
            SECTION,
            format!("{label}: it ends on the final chain's tip {}", last.tip),
        );
    } else {
        cases.fail(
            SECTION,
            label,
            format!("its last block is {tip:?}, the final chain's {want}"),
        );
    }
    let heartwood = mixed::heartwood(run.manifest)
        .iter()
        .any(|n| n.name == "node6");
    if heartwood {
        cases.record(
            SECTION,
            "node6 is a Heartwood relay: its round rows are compared with the corpus by the store check",
        );
        return Ok(());
    }
    let dirs = run::archives(&run.run_dir.join("archives").join("node6"))?;
    if dirs.is_empty() {
        cases.fail(SECTION, "node6: its round rows", "no archive of node6");
        return Ok(());
    }
    let scratch = ScratchDir::new("live-resync")?;
    let mine_dir = scratch.path().join("node6");
    corpus::import(&dirs, &mine_dir, &run.manifest.seed)
        .map_err(|e| Error::Usage(format!("the archive of node6 does not import: {e}")))?;
    let mine = Corpus::load(&mine_dir)?;
    let rows = rows_of(&mine_dir)?;
    let expected = fresh::expected(run, "node1", &last.dir, last.tip)?;
    let problems = fresh::row_problems(&rows, &expected, mine.tip());
    if problems.is_empty() {
        cases.pass(
            SECTION,
            format!(
                "node6: its round rows and missed rows equal {} for every round",
                expected.from
            ),
        );
    } else {
        for p in problems {
            cases.fail(SECTION, "node6: its round rows", p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_read_under_either_spelling() {
        let v = json!({"height": 265, "highestRound": 5, "missedSlots": 8, "transactionCount": 41});
        let file = json!({"tip": {"height": 106, "id": "ab"}, "highestRound": 2, "missedRows": 0,
            "transactions": 149, "ok": true});
        assert_eq!(
            Summary::of(&file),
            Some(Summary {
                tip: 106,
                round: 2,
                missed: 0,
                transactions: 149
            })
        );
        assert_eq!(
            Summary::of(&v),
            Some(Summary {
                tip: 265,
                round: 5,
                missed: 8,
                transactions: 41
            })
        );
        assert_eq!(Summary::of(&json!({"tip": 1})), None);
    }

    #[test]
    fn the_backfill_explains_only_its_own_rows() {
        assert_eq!(compare_missed(8, 8, 3), Missed::Equal);
        assert_eq!(compare_missed(11, 8, 3), Missed::Backfill(3));
        assert_eq!(compare_missed(11, 8, 0), Missed::Differ);
        assert_eq!(compare_missed(10, 8, 3), Missed::Differ);
        assert_eq!(compare_missed(5, 8, 3), Missed::Differ);
    }
}
