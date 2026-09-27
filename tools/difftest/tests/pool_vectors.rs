//! L0 over the pool's golden vectors (class V21, installed in `packages/pool/tests/vectors`): every
//! scripted pool session, step by step, against Heartwood's pool, with the pool's message rules
//! (`levels::pool`). The devnet configuration comes from the crypto crate's vectors.
//!
//! In a tree whose pool is still a stub, every session is skipped; once the pool is built, every
//! session must be compared or be a documented difference.

use std::path::Path;

use heartwood_difftest::levels::l0;
use heartwood_difftest::report::Outcome;
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;

fn vectors(crate_name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages")
        .join(crate_name)
        .join("tests/vectors")
}

#[test]
fn the_pool_vectors_pass_l0() {
    let dir = vectors("pool");
    if !dir.join("MANIFEST.sha256").exists() {
        eprintln!("no pool vectors installed yet (make vectors-install)");
        return;
    }
    let set = VectorSet::load(&dir).expect("the pool vectors load");
    let crypto = VectorSet::load(&vectors("crypto")).expect("the crypto vectors load");
    let devnet = Devnet::from_vectors(&crypto)
        .expect("one devnet configuration")
        .expect("a genesis class");
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    assert!(report.is_success(), "{report}");
    assert!(report.without_subject().is_empty(), "{report}");
    let skipped_as_unbuilt = report
        .results
        .iter()
        .filter(|r| matches!(&r.outcome, Outcome::Skipped(why) if why.contains("not built")))
        .count();
    if skipped_as_unbuilt == set.record_count() {
        eprintln!("the pool is a stub in this tree: {skipped_as_unbuilt} sessions skipped");
        return;
    }
    assert_eq!(report.skipped(), 0, "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    assert!(report.compared() > 0, "{report}");
}
