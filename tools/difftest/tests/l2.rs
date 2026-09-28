//! L2 over the committed corpus: a chain of the reference devnet (native, one node: every kept
//! transaction type, resignations and a revoke, a restart, missed slots and secondary rewards),
//! replayed in Heartwood through its store with its revert plan and its restart plan, and compared
//! line by line with the digest lines and the restart lines the oracle's judge recorded for it,
//! with Heartwood's own checks (the schedule, the reward, the supply, and each delegate's latest
//! resignation height) and the store anchors. No oracle runs here.
//!
//! The same chain also goes through the processor and the job, as a follower imports it.
//!
//! In a tree whose store is still a stub, the replay runs in memory and the store's parts are
//! reported as skipped; once the store is built, nothing may be skipped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::digests;
use heartwood_difftest::levels::l2::{self, L2Options};
use heartwood_difftest::levels::l2_processor::{self, L2ProcessorOptions};
use heartwood_difftest::restart::{
    self, PlanKind, ReferenceMode, RestartOutcome, SAME_STATE, differing_keys,
};
use heartwood_difftest::store;
use serde_json::Value;

fn committed() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/txgen-one-node")
}

#[test]
fn the_committed_corpus_is_intact_and_judged() {
    let dir = committed();
    corpus::verify(&dir).expect("the manifest holds");
    let loaded = Corpus::load(&dir).expect("the corpus loads");
    assert!(
        loaded.tip() >= 5 * 53,
        "at least five rounds, tip {}",
        loaded.tip()
    );
    let (ok, judge) = l2::judge_report(&dir).expect("anchors.json");
    assert!(ok, "the judge's anchors hold: {judge}");
    assert_eq!(
        judge.pointer("/anchors/A6/ok"),
        Some(&Value::Bool(true)),
        "the judge's restarts reproduce the node's live restart"
    );
    let lines = digests::read(&dir.join(corpus::TS_DIGESTS)).expect("the digest lines");
    assert_eq!(
        lines.len(),
        1 + l2::actions(&loaded).len(),
        "one oracle line per step"
    );
    assert!(
        digests::revert_asymmetries(&lines).is_empty(),
        "{:?}",
        digests::revert_asymmetries(&lines)
    );
    let meta = &loaded.meta;
    assert!(meta.get("seed").and_then(|s| s.as_str()).is_some());
    assert!(!loaded.revert_plan.steps.is_empty());
}

#[test]
fn the_committed_corpus_has_a_restart_line_at_every_height() {
    let dir = committed();
    let loaded = Corpus::load(&dir).expect("the corpus loads");
    let plan = loaded.restart_plan.clone().expect("a restart plan");
    assert_eq!(plan.kind, PlanKind::Every);
    assert_eq!(plan.heights, (1..=loaded.tip()).collect::<Vec<_>>());
    assert_eq!(
        loaded
            .live_restarts
            .iter()
            .map(|l| l.height)
            .collect::<Vec<_>>(),
        vec![177, 177],
        "the node's live restart, by both paths"
    );
    let (_, judge) = l2::judge_report(&dir).expect("anchors.json");
    assert_eq!(
        judge.pointer("/restarts/ok"),
        Some(&Value::Bool(true)),
        "{judge}"
    );
    let forward: BTreeMap<u64, Value> = digests::read(&dir.join(corpus::TS_DIGESTS))
        .expect("the digest lines")
        .into_iter()
        .filter(|l| matches!(l["step"].as_str(), Some("genesis" | "apply")))
        .map(|l| (l["h"].as_u64().unwrap_or(0), l))
        .collect();
    let records = restart::read_restarts(&dir.join(restart::TS_RESTARTS)).expect("restart lines");
    assert_eq!(records.len(), 2 * plan.heights.len());
    for mode in ReferenceMode::ALL {
        let heights: Vec<u32> = records
            .iter()
            .filter(|r| r.mode == mode)
            .map(|r| r.height)
            .collect();
        assert_eq!(heights, plan.heights, "{}", mode.name());
    }
    for record in &records {
        let RestartOutcome::Line(line) = &record.outcome else {
            panic!("the reference refused a restart: {record:?}");
        };
        let want = &forward[&u64::from(record.height)];
        assert!(
            differing_keys(line, want, &SAME_STATE).is_empty(),
            "{} restart at {}",
            record.mode.name(),
            record.height
        );
    }
}

#[test]
fn the_committed_corpus_replays_in_heartwood() {
    let loaded = Corpus::load(&committed()).expect("the corpus loads");
    let built = store::store_is_built();
    let options = L2Options {
        restart: built,
        ..L2Options::default()
    };
    let report = l2::run(&loaded, &options, None).expect("the replay runs");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.compared(), report.passed(), "{report}");
    if built {
        assert_eq!(report.skipped(), 0, "{report}");
        let restarts = report
            .results
            .iter()
            .filter(|r| r.op == "restart" && r.label.starts_with("restart at"))
            .count();
        assert_eq!(restarts, 2 * 265, "{report}");
    } else {
        eprintln!("the store is a stub in this tree: the replay ran in memory");
    }
}

/// The committed chain replayed as a follower imports it: the download path, the enqueueing's
/// cuts and one block per job, the job and the block processor, committed per job; both cuts
/// against the judge's lines and its recorded processor results, and the store anchors. No oracle
/// runs, and no part of the replay may be skipped.
#[test]
fn the_committed_corpus_replays_through_the_processor() {
    let loaded = Corpus::load(&committed()).expect("the corpus loads");
    let options = L2ProcessorOptions {
        chunks: None,
        to: None,
        synthetic: false,
    };
    let report = l2_processor::run(&loaded, &options, None).expect("the replay runs");
    eprintln!("{report}");
    assert!(report.is_success(), "{report}");
    assert!(report.is_complete(), "{report}");
    assert!(report.compared() > 2 * 2_000, "{report}");
}
