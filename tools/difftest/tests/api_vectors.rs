//! L0 over the API's records of class V28 (`api.parseQuery`, `api.paginate`), without an oracle.
//!
//! Each record is a query string parsed as the reference's server parses it, or a list's path, query and count and the `meta` of its envelope.
//!
//! The class is read from `packages/api/tests/vectors`, or from the directory that `NODE_VECTORS` names (relative
//! to the workspace root), for example `target/vectors/local`. When the class is in neither, the
//! test says so and passes. Records of other operations of the class are left to their own test.

use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{MANIFEST, VectorSet};

/// The class.
const CLASS: &str = "V28-node";

/// The operations this test compares.
const OPS: [&str; 2] = ["api.parseQuery", "api.paginate"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the class is read from.
fn vector_dir() -> PathBuf {
    match std::env::var_os("NODE_VECTORS") {
        Some(dir) if !dir.is_empty() => workspace().join(dir),
        _ => workspace().join("packages/api/tests/vectors"),
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
fn the_api_vectors_pass_l0() {
    let dir = vector_dir();
    if !dir.join(MANIFEST).exists() {
        eprintln!(
            "the_api_vectors_pass_l0: no vector set in {}",
            dir.display()
        );
        return;
    }
    let mut set = VectorSet::load(&dir).expect("the vector set loads");
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    if set.files.is_empty() {
        eprintln!(
            "the_api_vectors_pass_l0: {CLASS} is not installed in {}",
            dir.display()
        );
        return;
    }
    for file in &mut set.files {
        file.records
            .retain(|record| OPS.contains(&record.op.as_str()));
    }
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{CLASS} ({}):\n{report}", OPS.join(", "));
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
}
