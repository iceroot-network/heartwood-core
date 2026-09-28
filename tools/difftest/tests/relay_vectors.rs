//! L0 over the relay's forging vectors (V30: the forging quorum and the round the relay answers
//! with) and the server's (V31: the request chain, the limiter and the served status), without an
//! oracle.
//!
//! Every record goes, by its operation, to the subject of its area. A record of an operation this
//! test does not list fails it.
//!
//! The classes are read from `packages/p2p/tests/vectors`, where `make vectors-install` puts them
//! and where they must be: a class that is not installed there fails the test. The directory that
//! `P2P_VECTORS` names (relative to the workspace root), for example `target/vectors/local`, is
//! read instead when it is set; it may lack a class, and the test then says so and passes. No
//! record may fail and none may be skipped. The only documented difference is the wallet shape of
//! the relay's round answer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject::{self, Documented};
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, VectorSet};

/// Each class with its operations, and the fewest records of each that the committed class has.
const CLASSES: [(&str, &[(&str, usize)]); 2] = [
    (
        "V30-relay-forger",
        &[("forger.round", 5), ("quorum.analyze", 134)],
    ),
    (
        "V31-server",
        &[
            ("p2p.serveRequest", 157),
            ("ratelimit.run", 19),
            ("status.serve", 7),
        ],
    ),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the classes are read from.
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
fn the_relay_and_server_vectors_pass_l0() {
    let dir = vector_dir();
    for (class, ops) in CLASSES {
        let Some(mut set) = dir.load(class).expect("the class is installed") else {
            eprintln!(
                "the_relay_and_server_vectors_pass_l0: {class} is not in {}, which P2P_VECTORS names",
                dir.path().display()
            );
            continue;
        };
        let devnet = devnet(&set);
        set.files.retain(|file| file.meta.class == class);
        let mut per_op: BTreeMap<&str, usize> = BTreeMap::new();
        for record in set.files.iter().flat_map(|file| &file.records) {
            assert!(
                ops.iter().any(|(op, _)| *op == record.op),
                "{class} has a record of {}, no operation of this test",
                record.op
            );
            *per_op.entry(record.op.as_str()).or_default() += 1;
        }
        for (op, fewest) in ops {
            let count = per_op.get(op).copied().unwrap_or(0);
            assert!(
                count >= *fewest,
                "{class} has {count} {op} records, fewer than {fewest}"
            );
        }
        let report = l0::run(&set, &subject::heartwood(Some(devnet)));
        eprintln!("{class}: records per operation {per_op:?}\n{report}");
        assert!(report.is_success(), "{report}");
        assert_eq!(report.skipped(), 0, "{report}");
        let allowed = [Documented::RoundWallets].map(Documented::reason);
        for reason in report.documented_by_reason().keys() {
            assert!(allowed.contains(reason), "{report}");
        }
    }
}
