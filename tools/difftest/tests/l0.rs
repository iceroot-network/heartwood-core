//! L0 over the committed golden vectors of `heartwood-crypto`: every record that a subject
//! compares must pass, and every record is compared or a documented difference; none is skipped.

use std::path::Path;

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;

#[test]
fn the_committed_vectors_pass_l0() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
    let set = VectorSet::load(&dir).expect("the committed vectors load");
    let devnet = Devnet::from_vectors(&set).expect("one devnet configuration");
    assert!(
        devnet.is_some(),
        "the committed vectors have a genesis class"
    );
    let report = l0::run(&set, &subject::heartwood(devnet));
    assert!(report.is_success(), "{report}");
    assert!(report.is_complete(), "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    // The counts change only on purpose: a new subject, a new documented difference, or new
    // vectors.
    assert_eq!(
        (report.compared(), report.documented()),
        (2212, 64),
        "{report}"
    );
}
