//! L0 over the committed golden vectors of `heartwood-p2p-wire` (class V13), without an oracle:
//! every record is compared and equal to the reference's result, none is skipped, and the only
//! documented differences are the records that assert them on both sides.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::vectors::{MANIFEST, VectorSet};

/// The vector class of the wire codec, the only class in its directory.
const CLASS: &str = "V13-p2p-wire";

/// The pinned counts of an L0 run over the committed wire vectors: the records compared (all
/// equal to the reference's result), and for each documented difference the records that assert
/// it. They change only on purpose: a new subject, a new documented difference, or new vectors.
/// `None` while the class is not installed.
const PINNED: Option<(usize, [(Documented, usize); 3])> = None;

fn vector_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/p2p-wire/tests/vectors")
}

#[test]
fn the_committed_wire_vectors_pass_l0() {
    let dir = vector_dir();
    if PINNED.is_none() && !dir.join(MANIFEST).exists() {
        // Nothing is installed and nothing is pinned.
        return;
    }
    let set = VectorSet::load(&dir).expect("the committed wire vectors load");
    for file in &set.files {
        assert_eq!(file.meta.class, CLASS, "{}", file.path.display());
    }
    // The class is self-contained: its records carry their own configuration, so no devnet.
    let report = l0::run(&set, &subject::heartwood(None));
    let Some((compared, documented)) = PINNED else {
        panic!("the wire vectors are installed: pin the counts of this run in PINNED\n{report}");
    };
    assert!(report.is_success(), "{report}");
    assert!(report.without_subject().is_empty(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    assert_eq!(
        report.compared() + report.documented(),
        set.record_count(),
        "{report}"
    );
    let documented: BTreeMap<&str, usize> = documented
        .iter()
        .map(|&(difference, records)| (difference.reason(), records))
        .collect();
    assert_eq!(report.documented_by_reason(), documented, "{report}");
    assert_eq!(report.compared(), compared, "{report}");
}
