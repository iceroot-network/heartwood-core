//! L0 over the peer network's vector class (V26: the common-block search, the peer verifier, the
//! status attestations, the monitor's decisions and a listed block's path to a block), without an
//! oracle.
//!
//! Every record goes, by its operation, to the subject of its area: the attestations
//! (`peer.attest`), the verifier (`nsect.find`, `verifier.checkState`) or the monitor
//! (`monitor.networkHeight`, `monitor.forkDecision`, `monitor.downloadPlan`, `block.fromListed`).
//! A record of any other operation fails the test. No record may fail; a record whose subject is
//! not written yet is listed, not failed, and the only documented difference is the unlinked fork
//! segment.
//!
//! The class is read from `packages/p2p/tests/vectors`, or from the directory that `P2P_VECTORS`
//! names (relative to the workspace root), for example `target/vectors/local`. When the class is
//! in neither, the test says so and passes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{MANIFEST, VectorSet};

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
fn vector_dir() -> PathBuf {
    match std::env::var_os("P2P_VECTORS") {
        Some(dir) if !dir.is_empty() => workspace().join(dir),
        _ => workspace().join("packages/p2p/tests/vectors"),
    }
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
    if !dir.join(MANIFEST).exists() {
        eprintln!(
            "the_network_vectors_pass_l0: no vector set in {}",
            dir.display()
        );
        return;
    }
    let mut set = VectorSet::load(&dir).expect("the vector set loads");
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    if set.files.is_empty() {
        eprintln!(
            "the_network_vectors_pass_l0: {CLASS} is not installed in {}",
            dir.display()
        );
        return;
    }
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
    let unlinked = Documented::UnlinkedForkSegment.reason();
    for reason in report.documented_by_reason().keys() {
        assert_eq!(*reason, unlinked, "{report}");
    }
}
