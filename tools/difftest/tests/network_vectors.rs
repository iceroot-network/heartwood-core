//! L0 over the peer network's vector class (V26: the common-block search, the peer verifier, the
//! status attestations, the monitor's decisions and a listed block's path to a block), without an
//! oracle.
//!
//! Every record goes, by its operation, to the subject of its area: the attestations
//! (`peer.attest`), the verifier (`nsect.find`, `verifier.checkState`) or the monitor
//! (`monitor.networkHeight`, `monitor.forkDecision`, `monitor.downloadPlan`, `block.fromListed`).
//! A record of any other operation fails the test. No record may fail; a record whose subject is
//! not written yet is listed, not failed, and the only documented differences are the unlinked
//! fork segment and, in a listed block, a transaction of a removed type or with a memo that is
//! not UTF-8.
//!
//! The class is read from `packages/p2p/tests/vectors`, where `make vectors-install` puts it and
//! where it must be: a class that is not installed there fails the test. The directory that
//! `P2P_VECTORS` names (relative to the workspace root), for example `target/vectors/local`, is
//! read instead when it is set; it may lack the class, and the test then says so and passes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, VectorSet};

/// The class of the peer network's vectors.
const CLASS: &str = "V26-network";

/// The area of each operation of the class.
const AREAS: [(&str, &str); 7] = [
    ("peer.attest", "attestations"),
    ("nsect.find", "verifier"),
    ("verifier.checkState", "verifier"),
    ("monitor.networkHeight", "monitor"),
    ("monitor.forkDecision", "monitor"),
    ("monitor.downloadPlan", "monitor"),
    ("block.fromListed", "monitor"),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the class is read from.
fn vector_dir() -> ClassDir {
    ClassDir::from_env(&workspace(), "P2P_VECTORS", "packages/p2p/tests/vectors")
}

/// The devnet configuration: from the directory's genesis class when it has one (a whole run),
/// else from the crypto crate's committed vectors.
fn devnet(set: &VectorSet) -> Devnet {
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
fn the_network_vectors_pass_l0() {
    let dir = vector_dir();
    let Some(mut set) = dir.load(CLASS).expect("the class is installed") else {
        eprintln!(
            "the_network_vectors_pass_l0: {CLASS} is not in {}, which P2P_VECTORS names",
            dir.path().display()
        );
        return;
    };
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    let areas: BTreeMap<&str, &str> = AREAS.into_iter().collect();
    let mut per_area: BTreeMap<&str, usize> = BTreeMap::new();
    for record in set.files.iter().flat_map(|file| &file.records) {
        let area = areas.get(record.op.as_str()).unwrap_or_else(|| {
            panic!("{CLASS} has a record of {}, no area's operation", record.op)
        });
        *per_area.entry(area).or_default() += 1;
    }
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{CLASS}: records per area {per_area:?}\n{report}");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    // Besides the unlinked fork segment, a listed block may hold a transaction of a removed
    // type or with a memo that is not UTF-8, which the transaction reader refuses.
    let allowed = [
        Documented::UnlinkedForkSegment,
        Documented::RemovedType,
        Documented::NonUtf8Memo,
    ]
    .map(Documented::reason);
    for reason in report.documented_by_reason().keys() {
        assert!(allowed.contains(reason), "{report}");
    }
}
