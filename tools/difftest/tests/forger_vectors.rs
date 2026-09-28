//! L0 over the forger's vector class (V29: a block forged from its inputs, a slot's decision and
//! the collation of the relay's answers), without an oracle.
//!
//! Every record goes, by its operation, to the subject of its area: `forger.forge`,
//! `forger.decide`, `forger.collateAndForge`, and the relay's answers the forger reads
//! (`internal.currentRound`, `internal.networkState`).
//!
//! The class is read from `packages/forger/tests/vectors`, where `make vectors-install` puts it
//! and where it must be: a class that is not installed there fails the test. The directory that
//! `FORGER_VECTORS` names (relative to the workspace root), for example `target/vectors/local`,
//! is read instead when it is set; it may lack the class, and the test then says so and passes. No record may fail, none
//! may be skipped, and every operation of the class must have records.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::vectors::{ClassDir, VectorSet};

/// The class.
const CLASS: &str = "V29-forging";

/// The operations the class holds, each with the fewest records the committed class has.
const OPS: [(&str, usize); 5] = [
    ("forger.forge", 29),
    ("forger.collateAndForge", 7),
    ("forger.decide", 42),
    ("internal.currentRound", 4),
    ("internal.networkState", 21),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the class is read from.
fn vector_dir() -> ClassDir {
    ClassDir::from_env(
        &workspace(),
        "FORGER_VECTORS",
        "packages/forger/tests/vectors",
    )
}

/// The devnet configuration: from the directory's genesis class when it has one (a whole run),
/// else from the crypto crate's committed vectors.
fn devnet(set: &VectorSet) -> heartwood_difftest::subjects::Devnet {
    use heartwood_difftest::subjects::Devnet;
    if let Some(devnet) = Devnet::from_vectors(set).expect("one devnet configuration") {
        return devnet;
    }
    let crypto = VectorSet::load(&workspace().join("packages/crypto/tests/vectors"))
        .expect("the crypto vectors load");
    Devnet::from_vectors(&crypto)
        .expect("one devnet configuration")
        .expect("the crypto vectors have a genesis class")
}

#[test]
fn the_forging_vectors_pass_l0() {
    let dir = vector_dir();
    let Some(mut set) = dir.load(CLASS).expect("the class is installed") else {
        eprintln!(
            "the_forging_vectors_pass_l0: {CLASS} is not in {}, which FORGER_VECTORS names",
            dir.path().display()
        );
        return;
    };
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    let mut per_op: BTreeMap<&str, usize> = BTreeMap::new();
    for record in set.files.iter().flat_map(|file| &file.records) {
        assert!(
            OPS.iter().any(|(op, _)| *op == record.op),
            "{CLASS} has a record of {}, no operation of this test",
            record.op
        );
        *per_op.entry(record.op.as_str()).or_default() += 1;
    }
    for (op, fewest) in OPS {
        let count = per_op.get(op).copied().unwrap_or(0);
        assert!(
            count >= fewest,
            "{CLASS} has {count} {op} records, fewer than {fewest}"
        );
    }
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{CLASS}: records per operation {per_op:?}\n{report}");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    // The one documented difference: the forging delegate's own key on a peer's over-height
    // header.
    let allowed = [Documented::OverHeightOwnBlock].map(Documented::reason);
    for reason in report.documented_by_reason().keys() {
        assert!(allowed.contains(reason), "{report}");
    }
}
