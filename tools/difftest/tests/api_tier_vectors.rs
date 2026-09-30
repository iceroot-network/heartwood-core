//! L0 over the API tier's records of class V34: the state's index orders, the listings, the
//! pagination and its tree, the wallet search and the collation, the SQL records, the chain
//! routes' documents (the OpenAPI document, the transaction schemas, the fee statistics and the
//! query fuzz target's inputs) and the productivity's window, without an oracle.
//!
//! Every record goes, by its operation, to the subject of its area. A record of an operation this
//! test does not list fails it, and so does a listed operation with fewer records than its
//! minimum, as `tools/oracle/vector-minimums.json` gives them (read at run time).
//!
//! The class is read from `packages/api/tests/vectors`, where `make vectors-install` puts it and
//! where it must be: a class that is not installed there fails the test. The directory that
//! `API_TIER_VECTORS` names (relative to the workspace root), for example `target/vectors/local`,
//! is read instead when it is set; it may lack the class, and the test then says so and passes.
//! An operation that such a run has no record of at all is reported and left out, so that a run
//! of part of the tier can be checked; an operation with some records, but fewer than its
//! minimum, fails. No record may fail and none may be skipped. The documented differences are the
//! fee statistics' registered types and the document's kept routes, which the live comparisons
//! judge and the records (written from the oracle's kept-only output) do not reach.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use heartwood_difftest::levels::l0;
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, VectorSet};

/// The class.
const CLASS: &str = "V34-api-tiers";

/// The oracle's manifest of the fewest records of each operation, per class.
const MINIMUMS: &str = "tools/oracle/vector-minimums.json";

/// The operations of the class with the fewest records of each, from the oracle's manifest, which
/// `make vectors-install` checks too.
fn minimums() -> Vec<(String, usize)> {
    let path = workspace().join(MINIMUMS);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
    let class = manifest[CLASS]
        .as_object()
        .unwrap_or_else(|| panic!("{} lists no {CLASS}", path.display()));
    let minimums: Vec<(String, usize)> = class
        .iter()
        .map(|(op, fewest)| {
            let fewest = fewest
                .as_u64()
                .and_then(|fewest| usize::try_from(fewest).ok())
                .unwrap_or_else(|| panic!("{op}'s minimum in {} is not a count", path.display()));
            (op.clone(), fewest)
        })
        .collect();
    assert!(
        !minimums.is_empty(),
        "{} lists no operation of {CLASS}",
        path.display()
    );
    minimums
}

/// Operations the class may hold besides the minimums' (records the parts add beyond them): the
/// query fuzz target's inputs and the SQL records' provenance.
const EXTRA: [&str; 2] = ["api.fuzzQuery", "sql.meta"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the class is read from.
fn vector_dir() -> ClassDir {
    ClassDir::from_env(
        &workspace(),
        "API_TIER_VECTORS",
        "packages/api/tests/vectors",
    )
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
fn the_api_tier_vectors_pass_l0() {
    let minimums = minimums();
    let dir = vector_dir();
    let named = matches!(dir, ClassDir::Named(_));
    let Some(mut set) = dir.load(CLASS).expect("the class is installed") else {
        eprintln!(
            "the_api_tier_vectors_pass_l0: {CLASS} is not in {}, which API_TIER_VECTORS names",
            dir.path().display()
        );
        return;
    };
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    let mut per_op: BTreeMap<&str, usize> = BTreeMap::new();
    for record in set.files.iter().flat_map(|file| &file.records) {
        assert!(
            minimums.iter().any(|(op, _)| *op == record.op) || EXTRA.contains(&record.op.as_str()),
            "{CLASS} has a record of {}, no operation of this test",
            record.op
        );
        *per_op.entry(record.op.as_str()).or_default() += 1;
    }
    for (op, fewest) in &minimums {
        let count = per_op.get(op.as_str()).copied().unwrap_or(0);
        if named && count == 0 {
            eprintln!("the_api_tier_vectors_pass_l0: the run has no {op} record");
            continue;
        }
        assert!(
            count >= *fewest,
            "{CLASS} has {count} {op} records, fewer than {fewest}"
        );
    }
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{CLASS}: records per operation {per_op:?}\n{report}");
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
}
