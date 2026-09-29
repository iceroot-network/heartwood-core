//! The synthetic cases of the node's own forger's blocks on a corpus chain (`run l2
//! --synthetic`). Each runs through the reference's receipt and job (`chain.job` with
//! `fromForger` and `receive`) and through Heartwood's blockchain service ([`ServiceRig`]): the
//! service's own lateness rules, its job, its broadcast, its fork and its recovery. The answers,
//! the log lines and the digest lines are compared. The case's block is the corpus's last one, on
//! the chain before it:
//!
//! - `forger-received-late`: the block from the forger received with 1,000 ms left in its slot
//!   (the reference reads whole seconds, so any time left of 1,000 ms or less is too little) is
//!   discarded: `Discarded block <h> because it was received too late`;
//! - `forger-received-in-time`: received with 1,001 ms left, which the reference reads as 2,000
//!   ms, the block is accepted and broadcast;
//! - `forger-processed-late`: received with 7 s left and processed with 1,000 ms left, it is
//!   accepted, not broadcast, logged `Discarded block <h> because it was processed too late` and
//!   marked as forked with one block to roll back; the service's recovery reverts the block and
//!   the block's return (its re-apply) gives equal lines on both sides;
//! - `forger-posted-twice`: the same bytes posted twice in time; the first is accepted and
//!   broadcast, and the second job is dropped as applied already (its first height is not above
//!   the tip): no processing and no second broadcast.

use heartwood_crypto::crypto::slots;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Block, Milestones};
use heartwood_kernel::config::crypto::CryptoTexts;
use heartwood_state::digest::Step;
use serde_json::{Value, json};

use crate::compare;
use crate::corpus::Corpus;
use crate::digests;
use crate::errors::{Error, Result};
use crate::levels::l2::{call, corpus_crypto};
use crate::levels::l2_processor::{JobBlock, JobRig, crypto_of};
use crate::oracle::OracleServer;
use crate::report::{CaseResult, Outcome};
use crate::service_rig::{Posted, ServiceRig};

/// The cases, in order.
pub const CASES: [&str; 4] = [
    "forger-received-late",
    "forger-received-in-time",
    "forger-processed-late",
    "forger-posted-twice",
];

/// The time left in the slot that the reference reads as too little: 1,000 ms.
const LATE_LEFT_MS: i64 = 1_000;
/// The time left that the reference still reads as in time: 1,001 ms, which it reads as 2,000.
const JUST_IN_TIME_LEFT_MS: i64 = 1_001;
/// The time into the slot of a block received in time.
const IN_TIME_MS: i64 = 1_000;

/// One case's run on both sides.
struct Case<'a> {
    corpus: &'a Corpus,
    name: &'static str,
    session: Value,
    rig: ServiceRig,
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
        let block = Block::receive(&last.bytes, &corpus.milestones)
            .map_err(|e| Error::Oracle(format!("block {}: {e}", last.height)))?;
        let slot = slot_of(&block, &corpus.milestones);
        let rig = ServiceRig::start(rig(corpus, prefix)?.into_parts(), slot.0)
            .map_err(|why| Error::Oracle(format!("the service does not start: {why}")))?;
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

    /// The block posted by the forger and processed at `now` by Heartwood's service.
    fn post(
        &mut self,
        received_at: i64,
        now: i64,
        step: Step,
    ) -> std::result::Result<Posted, String> {
        self.rig.post_forged(&self.block, received_at, now, step)
    }

    /// Record the oracle's answer against the service's, lines included, and whether the service
    /// broadcast as the oracle's answer says.
    fn record_post(
        &mut self,
        what: &str,
        oracle: &Value,
        ours: std::result::Result<Posted, String>,
    ) {
        match ours {
            Ok(posted) => {
                let differences = compare::diff(oracle, &posted.value);
                let outcome = if differences.is_empty() {
                    Outcome::Pass
                } else {
                    Outcome::Mismatch(differences)
                };
                self.record(what, outcome);
            }
            Err(why) => self.record(what, Outcome::HarnessError(why)),
        }
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

fn mismatch(compared: std::result::Result<Vec<compare::Difference>, String>) -> Outcome {
    match compared {
        Ok(differences) if differences.is_empty() => Outcome::Pass,
        Ok(differences) => Outcome::Mismatch(differences),
        Err(why) => Outcome::HarnessError(why),
    }
}

/// Whether the service broadcast the block as many times as `expected`.
fn broadcast_count(ours: &std::result::Result<Posted, String>, expected: usize) -> Outcome {
    match ours {
        Ok(posted) if posted.broadcasts.len() == expected => Outcome::Pass,
        Ok(posted) => Outcome::HarnessError(format!(
            "Heartwood broadcast {:?}, expected {expected} broadcasts",
            posted.broadcasts
        )),
        Err(why) => Outcome::HarnessError(why.clone()),
    }
}

/// `forger-received-late`.
fn received_late(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let now = case.slot.1 - LATE_LEFT_MS;
    let oracle = case.oracle_post(server, now, now)?;
    let ours = case.post(now, now, Step::Apply);
    case.record_post("the block is discarded, the line", &oracle, ours.clone());
    case.record("nothing is broadcast", broadcast_count(&ours, 0));
    Ok(())
}

/// `forger-received-in-time`.
fn received_in_time(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let now = case.slot.1 - JUST_IN_TIME_LEFT_MS;
    let oracle = case.oracle_post(server, now, now)?;
    let ours = case.post(now, now, Step::Apply);
    case.record_post("accepted and broadcast", &oracle, ours.clone());
    case.record("broadcast once", broadcast_count(&ours, 1));
    Ok(())
}

/// `forger-processed-late`.
fn processed_late(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let (received_at, now) = (case.slot.0 + IN_TIME_MS, case.slot.1 - LATE_LEFT_MS);
    let oracle = case.oracle_post(server, received_at, now)?;
    let ours = case.post(received_at, now, Step::Apply);
    case.record_post(
        "processed, forked with one block to roll back, the lines",
        &oracle,
        ours.clone(),
    );
    case.record("nothing is broadcast", broadcast_count(&ours, 0));
    // The service's recovery removes the block; the reference's is asked to do the same.
    let refreshes = case.rig.refreshes() + 1;
    let settled = case.rig.settle(refreshes);
    let theirs = call(
        server,
        "chain.revert",
        u64::from(case.height),
        &json!({"session": case.session, "count": 1}),
    )?;
    let compared = match settled {
        Ok(()) => {
            let reverts = case.rig.take_reverts();
            let lines: Vec<Value> = reverts.iter().map(digests::line_value).collect();
            Ok(compare::diff(&theirs["lines"], &Value::Array(lines)))
        }
        Err(why) => Err(why),
    };
    case.record("the revert of one block", mismatch(compared));
    let args =
        json!({"session": case.session, "hex": hex::encode(&case.block.bytes), "step": "reapply"});
    let theirs = call(server, "chain.apply", u64::from(case.height), &args)?;
    let compared = case
        .post(received_at, received_at, Step::Reapply)
        .and_then(|posted| {
            let line = posted.seen.first().and_then(|seen| seen.line.as_ref());
            let line = line.ok_or("Heartwood wrote no line for the block's return")?;
            Ok(compare::diff(&theirs["line"], &digests::line_value(line)))
        });
    case.record("the block applied again", mismatch(compared));
    Ok(())
}

/// `forger-posted-twice`.
fn posted_twice(case: &mut Case<'_>, server: &mut OracleServer) -> Result<()> {
    let now = case.slot.0 + IN_TIME_MS;
    for (what, broadcasts) in [
        ("the first post", 1),
        ("the second post, equal to the last block", 0),
    ] {
        let oracle = case.oracle_post(server, now, now)?;
        let ours = case.post(now, now, Step::Apply);
        case.record_post(what, &oracle, ours.clone());
        case.record(
            &format!("{what}: broadcasts"),
            broadcast_count(&ours, broadcasts),
        );
    }
    Ok(())
}

/// Run every case on `corpus`'s chain.
pub fn run(corpus: &Corpus, oracle: &mut OracleServer) -> Result<Vec<CaseResult>> {
    let mut out = Vec::new();
    for name in CASES {
        let ran = Case::open(corpus, name, oracle).and_then(|mut case| {
            match name {
                "forger-received-late" => received_late(&mut case, oracle)?,
                "forger-received-in-time" => received_in_time(&mut case, oracle)?,
                "forger-processed-late" => processed_late(&mut case, oracle)?,
                _ => posted_twice(&mut case, oracle)?,
            }
            let _ = call(oracle, "chain.close", 1, &json!({"session": case.session}));
            let Case { rig, results, .. } = case;
            rig.stop();
            Ok(results)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed corpus's chain up to its last block, and that block's slot.
    fn service() -> (ServiceRig, JobBlock, (i64, i64)) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node");
        let corpus = Corpus::load(&dir).expect("the corpus loads");
        let (last, prefix) = corpus.blocks.split_last().expect("the corpus has blocks");
        let block = Block::receive(&last.bytes, &corpus.milestones).expect("a block");
        let slot = slot_of(&block, &corpus.milestones);
        let parts = rig(&corpus, prefix).expect("the rig starts").into_parts();
        let service = ServiceRig::start(parts, slot.0).expect("the service starts");
        (service, JobBlock::of(last.bytes.clone()), slot)
    }

    #[test]
    fn a_block_received_with_one_second_left_is_discarded_and_one_with_1001_ms_is_broadcast() {
        let (mut rig, block, slot) = service();
        let late = slot.1 - LATE_LEFT_MS;
        let posted = rig.post_forged(&block, late, late, Step::Apply).unwrap();
        assert_eq!(posted.value["received"], json!(false));
        assert_eq!(posted.value["lines"].as_array().map(Vec::len), Some(1));
        assert!(posted.broadcasts.is_empty() && posted.seen.is_empty());
        let early = slot.1 - JUST_IN_TIME_LEFT_MS;
        let posted = rig.post_forged(&block, early, early, Step::Apply).unwrap();
        assert_eq!(posted.broadcasts, [rig.tip().0]);
        assert_eq!(posted.value["end"], json!("broadcast"));
        rig.stop();
    }

    #[test]
    fn a_block_processed_late_is_forked_removed_and_never_broadcast() {
        let (mut rig, block, slot) = service();
        let (received, now) = (slot.0 + IN_TIME_MS, slot.1 - LATE_LEFT_MS);
        let tip = rig.tip().0;
        let posted = rig.post_forged(&block, received, now, Step::Apply).unwrap();
        assert!(posted.broadcasts.is_empty());
        assert_eq!(posted.value["end"], json!("fork"));
        assert_eq!(posted.value["rollback"], json!(1));
        let lines = crate::service_rig::lines_of(&posted.value);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("because it was processed too late"))
        );
        rig.settle(1).unwrap();
        assert_eq!(
            rig.take_reverts().len(),
            1,
            "the recovery reverts the block"
        );
        assert_eq!(
            rig.tip().0 + 1,
            tip + 1,
            "the chain is back below the block"
        );
        rig.stop();
    }
}
