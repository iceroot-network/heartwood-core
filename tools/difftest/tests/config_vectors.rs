//! L0 over the committed configuration vectors of `heartwood-kernel` (class V23), without an
//! oracle: every record is compared and equal to the reference's result, or a refusal of the
//! crypto files that only Heartwood makes, whose documented form holds on the record's files.

use std::collections::BTreeMap;
use std::path::Path;

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;

/// The vector class of the crypto files.
const CLASS: &str = "V23-config";

/// The pinned counts of an L0 run over V23: the records compared (both accept with the same
/// merged milestones, or both refuse), and for each documented difference the records that show
/// it. They change only on purpose: a new subject, a new documented difference, or new vectors.
const PINNED: (usize, [(Documented, usize); 2]) = (
    17,
    [
        (Documented::StrictSchema, 75),
        (Documented::FeeKeyValueType, 4),
    ],
);

#[test]
fn the_committed_configuration_vectors_pass_l0() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
    let crypto = VectorSet::load(&root.join("crypto/tests/vectors")).expect("the crypto vectors");
    let devnet = Devnet::from_vectors(&crypto).expect("one devnet configuration");
    let mut set = VectorSet::load(&root.join("kernel/tests/vectors")).expect("the kernel vectors");
    set.files.retain(|file| file.meta.class == CLASS);
    assert_eq!(set.files.len(), 1, "{CLASS} is installed");
    let report = l0::run(&set, &subject::heartwood(devnet));
    assert!(report.is_success(), "{report}");
    assert!(report.without_subject().is_empty(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    let (compared, documented) = PINNED;
    let documented: BTreeMap<&str, usize> = documented
        .iter()
        .map(|&(difference, records)| (difference.reason(), records))
        .collect();
    assert_eq!(report.documented_by_reason(), documented, "{report}");
    assert_eq!(report.compared(), compared, "{report}");
}
