//! L2 over the committed corpus: a chain of the reference devnet (native, one node: every kept
//! transaction type, resignations and a revoke, a restart, missed slots and secondary rewards),
//! replayed in Heartwood with its revert plan and compared line by line with the digest lines the
//! oracle's judge recorded for it. No oracle runs here.

use std::path::{Path, PathBuf};

use heartwood_difftest::corpus::{self, Corpus};
use heartwood_difftest::digests;
use heartwood_difftest::levels::l2::{self, L2Options};

fn committed() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus/c1-txgen")
}

#[test]
fn the_committed_corpus_is_intact_and_judged() {
    let dir = committed();
    corpus::verify(&dir).expect("the manifest holds");
    let loaded = Corpus::load(&dir).expect("the corpus loads");
    assert!(loaded.tip() >= 5 * 53, "at least five rounds, tip {}", loaded.tip());
    let (ok, judge) = l2::judge_report(&dir).expect("anchors.json");
    assert!(ok, "the judge's anchors hold: {judge}");
    let lines = digests::read(&dir.join(corpus::TS_DIGESTS)).expect("the digest lines");
    assert_eq!(lines.len(), 1 + l2::actions(&loaded).len(), "one oracle line per step");
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
fn the_committed_corpus_replays_in_heartwood() {
    let loaded = Corpus::load(&committed()).expect("the corpus loads");
    if !l2::machine_available(&loaded) {
        eprintln!("the state machine of heartwood-state is not built in this tree: nothing replayed");
        return;
    }
    let report = l2::run(&loaded, &L2Options::default(), None).expect("the replay runs");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.compared(), report.passed(), "{report}");
}
