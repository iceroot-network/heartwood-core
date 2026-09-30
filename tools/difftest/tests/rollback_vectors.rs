//! L0 over the rollback's records of class V33 (`chain.rollback` on the committed one-node corpus:
//! every refusal with its lines, and each rollback with its target, lines, deleted rows and store
//! summary, then the start after it with its line, summary and the next round's rows), without an
//! oracle.
//!
//! The class is read from `packages/blockchain/tests/vectors`, where `make vectors-install` puts it
//! and where it must be: a class that is not installed there fails the test. The directory that
//! `NODE_VECTORS` names (relative to the workspace root), for example `target/vectors/local`, is
//! read instead when it is set; it may lack the class, and the test then says so and passes.

use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::vectors::ClassDir;

/// The class.
const CLASS: &str = "V33-rollback";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_rollback_vectors_pass_l0() {
    let dir = ClassDir::from_env(
        &workspace(),
        "NODE_VECTORS",
        "packages/blockchain/tests/vectors",
    );
    let Some(mut set) = dir.load(CLASS).expect("the class is installed") else {
        eprintln!(
            "the_rollback_vectors_pass_l0: {CLASS} is not in {}, which NODE_VECTORS names",
            dir.path().display()
        );
        return;
    };
    set.files.retain(|file| file.meta.class == CLASS);
    let records: usize = set.files.iter().map(|f| f.records.len()).sum();
    assert!(records >= 29, "{CLASS} holds {records} records");
    let report = l0::run(&set, &subject::heartwood(None));
    eprintln!("{CLASS}:\n{report}");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
}
