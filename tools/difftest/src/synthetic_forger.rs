//! The synthetic cases of the node's own forger's blocks on a corpus chain (`run l2
//! --synthetic`), each run through the reference's receipt and job (`chain.job` with
//! `fromForger` and `receive`) and Heartwood's lateness rule and job, their answers and lines
//! compared. The case's block is the corpus's last one, on the chain before it:
//!
//! - `forger-received-late`: the block from the forger received with 1,000 ms left in its slot
//!   (the reference's whole seconds read any time left of 1,000 ms or less so) is discarded:
//!   `Discarded block <h> because it was received too late`;
//! - `forger-processed-late`: received with 7 s left and processed with 1,000 ms left, it is
//!   accepted, not broadcast, logged `Discarded block <h> because it was processed too late` and
//!   marked as forked with one block to roll back; the recovery's revert of one block and the
//!   block's return (its re-apply) give equal lines on both sides;
//! - `forger-posted-twice`: the same bytes posted twice in time, the second an
//!   `EqualToLastBlock` broadcast again.
//!
//! Heartwood's side runs its job on a node at the chain's tip ([`JobRig`]), and applies to the
//! job's end the blockchain service's lateness rule ([`forger_blocks::too_late`]) as the service
//! applies it.

use heartwood_blockchain::forger_blocks;
use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::{hex, js};
use heartwood_crypto::{Block, Milestones};
use heartwood_kernel::config::crypto::CryptoTexts;
use heartwood_state::digest::Step;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::levels::l2_processor::{JobBlock, JobInputs, JobRig, RigJob, crypto_of, judge_job};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};

/// The cases, in order.
pub const CASES: [&str; 3] = [
    "forger-received-late",
    "forger-processed-late",
    "forger-posted-twice",
];

/// The time left in the slot that the reference reads as too little: 1,000 ms.
const LATE_LEFT_MS: i64 = 1_000;
/// The time into the slot of a block received in time.
const IN_TIME_MS: i64 = 1_000;

/// One case's run on both sides.
struct Case<'a> {
    corpus: &'a Corpus,
    name: &'static str,
    session: Value,
    rig: JobRig,
    block: JobBlock,
    height: u32,
    /// The block's slot, in milliseconds since the Unix epoch: its start and its end.
    slot: (i64, i64),
    results: Vec<CaseResult>,
}

impl<'a> Case<'a> {
    /// Both sides at the chain before the corpus's last block.
    fn open(corpus: &'a Corpus, name: &'static str, server: &mut OracleServer) -> Result<Case<'a>> {
        let Some((last, prefix)) = corpus.blocks.split_last() else {
            return Err(Error::Oracle("the corpus has no block to post".into()));
        };
        let opened = call(
            server,
            "chain.open",
            1,
            &json!({"crypto": corpus_crypto(corpus)?}),
        )?;
        let session = opened.get("session").cloned().unwrap_or(Value::Null);
        for block in prefix {
            let args = json!({"session": session, "hex": hex::encode(&block.bytes)});
            let applied = call(server, "chain.apply", u64::from(block.height), &args)?;
            if applied.get("accepted") != Some(&Value::Bool(true)) {
                let height = block.height;
                return Err(Error::Oracle(format!(
                    "the oracle refused block {height}: {applied}"
                )));
            }
        }
        let rig = rig(corpus, prefix)?;
        let block = Block::receive(&last.bytes, &corpus.milestones)
            .map_err(|e| Error::Oracle(format!("block {}: {e}", last.height)))?;
        let slot = slot_of(&block, &corpus.milestones);
        Ok(Case {
            corpus,
            name,
            session,
            rig,
            block: JobBlock::of(last.bytes.clone()),
            height: last.height,
            slot,
            results: Vec::new(),
        })
    }

    fn record(&mut self, what: &str, outcome: Outcome) {
        self.results.push(CaseResult {
            file: format!("{}/synthetic-forger", self.corpus.name()),
            line: self.results.len() + 1,
            op: "chain.job".to_owned(),
            label: format!("{}: {what}", self.name),
            height: u64::from(self.height),
            outcome,
        });
    }

    /// The block posted by the forger at `received_at` and processed at `now` on the oracle.
    fn oracle_post(&self, server: &mut OracleServer, received_at: i64, now: i64) -> Result<Value> {
        let hexes = json!([hex::encode(&self.block.bytes)]);
        let args = json!({"session": self.session, "hexes": hexes, "now": now,
            "fromForger": true, "receive": true, "receivedAt": received_at});
        call(server, "chain.job", u64::from(self.height), &args)
    }

    /// The block posted by the forger and processed at `now` on Heartwood's node: the job's run,
    /// with the service's answer to a late forger's block applied (not broadcast; forked with one
    /// block to roll back; the line).
    fn heartwood_post(
        &mut self,
        now: i64,
    ) -> std::result::Result<(RigJob, Option<String>), String> {
        let mut incoming = self.rig.enqueue(std::slice::from_ref(&self.block))?;
        incoming
            .iter_mut()
            .for_each(|block| block.from_forger = true);
        let blocks = [self.block.clone()];
        let ran = self
            .rig
            .run_enqueued(&blocks, incoming, &JobInputs::at(now), &[]);
        let ran = ran.map_err(|problem| format!("{problem:?}"))?;
        let RigJob::Ran {
            outcome,
            seen,
            mut value,
        } = ran
        else {
            return Ok((ran, None));
        };
        let broadcast = matches!(
            &outcome.end,
            heartwood_blockchain::JobEnd::Broadcast(_, true)
        );
        let (timestamp, milestones) = (self.slot_timestamp(), &self.corpus.milestones);
        let mut line = None;
        if broadcast && forger_blocks::too_late(timestamp, self.height, now, milestones) {
            let link = json!({"height": self.height, "id": value["block"]["id"]});
            value["end"] = json!("fork");
            value["broadcast"] = json!(false);
            value["forked"] = link;
            value["rollback"] = json!(1);
            line = Some(format!(
                "Discarded block {} because it was processed too late :exclamation:",
                locale(self.height)
            ));
        }
        Ok((
            RigJob::Ran {
                outcome,
                seen,
                value,
            },
            line,
        ))
    }

    fn slot_timestamp(&self) -> u32 {
        self.block.parts().map_or(0, |(header, _)| header.timestamp)
    }
}

/// Heartwood's node at the chain of `prefix`.
fn rig(corpus: &Corpus, prefix: &[crate::corpus::CorpusBlock]) -> Result<JobRig> {
    let read = |name: &str| std::fs::read_to_string(corpus.dir.join("crypto").join(name)).ok();
    let files = crypto_of(&CryptoTexts {
        network: read("network.json"),
        milestones: read("milestones.json"),
        genesis: read("genesisBlock.json"),
        exceptions: read("exceptions.json"),
    })
    .map_err(|problem| Error::Oracle(format!("{problem:?}")))?;
    let mut rig = JobRig::start(&files).map_err(|problem| Error::Oracle(format!("{problem:?}")))?;
    let blocks: Vec<Block> = (prefix.iter())
        .map(|block| Block::receive(&block.bytes, &corpus.milestones))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| Error::Oracle(format!("a corpus block: {e}")))?;
    rig.extend(&blocks)
        .map_err(|problem| Error::Oracle(format!("{problem:?}")))?;
    Ok(rig)
}

/// The slot of `block`: its start and its end, in milliseconds since the Unix epoch.
fn slot_of(block: &Block, milestones: &Milestones) -> (i64, i64) {
    let params = milestones.at(block.height());
    let epoch = slots::epoch_unix_ms(params.epoch()).unwrap_or(0);
    let block_ms = i64::try_from(params.block_time())
        .unwrap_or(8)
        .saturating_mul(1_000);
    let start = epoch.saturating_add(i64::from(block.header().timestamp).saturating_mul(1_000));
    (start, start.saturating_add(block_ms))
}

fn locale(height: u32) -> String {
    js::to_locale_integer(height.into())
}

/// Whether the oracle's `lines` hold `line`.
fn logged(oracle: &Value, line: &str) -> bool {
    let lines = oracle["lines"].as_array().cloned().unwrap_or_default();
    lines.iter().any(|logged| logged.as_str() == Some(line))
}

/// The oracle's job without its log lines, as the job's judge reads it.
fn without_lines(oracle: &Value) -> Value {
    let mut job = oracle.clone();
    if let Value::Object(fields) = &mut job {
        fields.shift_remove("lines");
    }
    job
}

fn outcome(pass: bool, why: impl FnOnce() -> String) -> Outcome {
    if pass {
        Outcome::Pass
    } else {
        Outcome::HarnessError(why())
    }
}

/// `forger-received-late`.
fn received_late(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let now = case.slot.1 - LATE_LEFT_MS;
    let oracle = case.oracle_post(server, now, now)?;
    let line = format!(
        "Discarded block {} because it was received too late :exclamation:",
        locale(case.height)
    );
    let ours = forger_blocks::too_late(
        case.slot_timestamp(),
        case.height,
        now,
        &case.corpus.milestones,
    );
    let theirs = oracle.get("received") == Some(&Value::Bool(false));
    case.record(
        "the block is discarded",
        outcome(ours && theirs, || {
            format!("Heartwood discards it: {ours}; the reference: {oracle}")
        }),
    );
    case.record(
        "the line",
        outcome(logged(&oracle, &line), || {
            format!("the reference logged {}", oracle["lines"])
        }),
    );
    Ok(())
}

/// `forger-processed-late`.
fn processed_late(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let (received_at, now) = (case.slot.0 + IN_TIME_MS, case.slot.1 - LATE_LEFT_MS);
    let oracle = case.oracle_post(server, received_at, now)?;
    let received = !forger_blocks::too_late(
        case.slot_timestamp(),
        case.height,
        received_at,
        &case.corpus.milestones,
    );
    case.record(
        "received in time",
        outcome(received && oracle.get("received").is_none(), || {
            format!("{oracle}")
        }),
    );
    match case.heartwood_post(now) {
        Ok((ours, line)) => {
            let judged = judge_job(
                std::slice::from_ref(&case.block),
                &without_lines(&oracle),
                &ours,
            );
            case.record("processed, forked with one block to roll back", judged);
            let same = line.as_deref().is_some_and(|line| logged(&oracle, line));
            case.record(
                "the line",
                outcome(same, || {
                    format!("Heartwood: {line:?}; the reference: {}", oracle["lines"])
                }),
            );
        }
        Err(why) => case.record("processed", Outcome::HarnessError(why)),
    }
    // The recovery removes the block, and the block comes back.
    let theirs = call(
        server,
        "chain.revert",
        u64::from(case.height),
        &json!({"session": case.session, "count": 1}),
    )?;
    let ours = case
        .rig
        .revert()
        .map(|_| case.rig.lines().last().map(digests::line_value));
    let compared = match ours {
        Ok(Some(line)) => Ok(compare::diff(&theirs["lines"], &json!([line]))),
        Ok(None) => Err("Heartwood wrote no revert line".to_owned()),
        Err(why) => Err(why),
    };
    case.record("the revert of one block", mismatch(compared));
    let args =
        json!({"session": case.session, "hex": hex::encode(&case.block.bytes), "step": "reapply"});
    let theirs = call(server, "chain.apply", u64::from(case.height), &args)?;
    let inputs = JobInputs::at(received_at);
    let ours = case
        .rig
        .run(std::slice::from_ref(&case.block), &inputs, &[Step::Reapply]);
    let compared = match ours {
        Ok(RigJob::Ran { seen, .. }) => {
            let line = seen.first().and_then(|seen| seen.line.as_ref());
            let line = line.map_or(Value::Null, digests::line_value);
            Ok(compare::diff(&theirs["line"], &line))
        }
        Ok(RigJob::Refused(why)) => Err(format!("Heartwood's download refused it: {why}")),
        Err(problem) => Err(format!("{problem:?}")),
    };
    case.record("the block applied again", mismatch(compared));
    Ok(())
}

/// `forger-posted-twice`.
fn posted_twice(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let now = case.slot.0 + IN_TIME_MS;
    for what in ["the first post", "the second post, equal to the last block"] {
        let oracle = case.oracle_post(server, now, now)?;
        match case.heartwood_post(now) {
            Ok((ours, _)) => {
                let judged = judge_job(
                    std::slice::from_ref(&case.block),
                    &without_lines(&oracle),
                    &ours,
                );
                case.record(what, judged);
            }
            Err(why) => case.record(what, Outcome::HarnessError(why)),
        }
    }
    Ok(())
}

fn mismatch(compared: std::result::Result<Vec<compare::Difference>, String>) -> Outcome {
    match compared {
        Ok(differences) if differences.is_empty() => Outcome::Pass,
        Ok(differences) => Outcome::Mismatch(differences),
        Err(why) => Outcome::HarnessError(why),
    }
}

/// Run every case on `corpus`'s chain.
pub fn run(corpus: &Corpus, oracle: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let mut out = Vec::new();
    for name in CASES {
        let ran = Case::open(corpus, name, oracle).and_then(|mut case| {
            match name {
                "forger-received-late" => received_late(&mut case, oracle)?,
                "forger-processed-late" => processed_late(&mut case, oracle)?,
                _ => posted_twice(&mut case, oracle)?,
            }
            let _ = call(oracle, "chain.close", 1, &json!({"session": case.session}));
            Ok(case.results)
        });
        match ran {
            Ok(results) => out.extend(results),
            Err(error) => out.push(CaseResult {
                file: format!("{}/synthetic-forger", corpus.name()),
                line: 0,
                op: "chain.job".to_owned(),
                label: name.to_owned(),
                height: u64::from(corpus.tip()),
                outcome: Outcome::HarnessError(error.to_string()),
            }),
        }
    }
    Ok(out)
}
