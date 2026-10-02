//! L0 over the pool's golden vectors (class V21, installed in `packages/pool/tests/vectors`): every
//! scripted pool session, step by step, against Heartwood's pool, with the pool's message rules
//! (`levels::pool`). The devnet configuration comes from the crypto crate's vectors.
//! Every session must be compared or be a documented difference.

use std::path::Path;

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, VectorSet};

fn vectors(crate_name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages")
        .join(crate_name)
        .join("tests/vectors")
}

#[test]
fn the_pool_vectors_pass_l0() {
    let set = ClassDir::Installed(vectors("pool"))
        .load("V21-pool")
        .expect("the pool vectors are installed")
        .expect("an installed directory holds its class");
    let crypto = VectorSet::load(&vectors("crypto")).expect("the crypto vectors load");
    let devnet = Devnet::from_vectors(&crypto)
        .expect("one devnet configuration")
        .expect("a genesis class");
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    assert!(report.is_success(), "{report}");
    assert!(report.without_subject().is_empty(), "{report}");
    assert_eq!(
        report.compared() + report.documented() + report.skipped(),
        set.record_count(),
        "{report}"
    );
    // The counts change only on purpose: a new documented difference, or new vectors.
    assert_eq!(
        (report.compared(), report.documented(), report.skipped()),
        (42, 14, 0),
        "{report}"
    );
}
