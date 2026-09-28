//! L0 over the processor's records of class V27 (`chain.process`: every reachable result of the
//! processor, each unchained status, and a refused block after missed slots), without an oracle;
//! and the reach of P13's cases through Heartwood's side alone.
//!
//! Each record is a block processed on a chain given whole; the missed rows of a block refused in
//! its apply are the documented difference its record asserts.
//!
//! The class is read from `packages/blockchain/tests/vectors`, where `make vectors-install` puts it
//! and where it must be: a class that is not installed there fails the test. The directory that
//! `NODE_VECTORS` names (relative to the workspace root), for example `target/vectors/local`, is
//! read instead when it is set; it may lack the class, and the test then says so and passes.
//! Records of other operations of the class are left to their own test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use heartwood_crypto::Network;
use heartwood_difftest::strategies::processor::processor_cases;
use heartwood_difftest::strategies::transactions::TxContext;
use heartwood_difftest::subject::{Case, Verdict};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

use heartwood_difftest::levels::l0;
use heartwood_difftest::levels::l1::{PROCESSOR_RESULTS, UNCHAINED_STATUSES};
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{ClassDir, VectorSet};

/// The class.
const CLASS: &str = "V27-processor";

/// The operations this test compares.
const OPS: [&str; 1] = ["chain.process"];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The directory the class is read from.
fn vector_dir() -> ClassDir {
    ClassDir::from_env(
        &workspace(),
        "NODE_VECTORS",
        "packages/blockchain/tests/vectors",
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
fn the_processor_vectors_pass_l0() {
    let dir = vector_dir();
    let Some(mut set) = dir.load(CLASS).expect("the class is installed") else {
        eprintln!(
            "the_processor_vectors_pass_l0: {CLASS} is not in {}, which NODE_VECTORS names",
            dir.path().display()
        );
        return;
    };
    let devnet = devnet(&set);
    set.files.retain(|file| file.meta.class == CLASS);
    for file in &mut set.files {
        file.records
            .retain(|record| OPS.contains(&record.op.as_str()));
    }
    let report = l0::run(&set, &subject::heartwood(Some(devnet)));
    eprintln!("{CLASS} ({}):\n{report}", OPS.join(", "));
    assert!(report.is_success(), "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
}

/// P13's cases reach every result the reference's processor can reach and every unchained status,
/// counted on Heartwood's side (L1 then requires the reference's answer to be the same, case by
/// case, and counts the reference's answers). `P13_CASES` sets the number of cases [400]; from
/// 1,000 cases each must be reached 20 times, as `make l1` asks of the reference's answers.
#[test]
fn the_processor_cases_reach_every_result() {
    let cases: usize = std::env::var("P13_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(400);
    let corpus = workspace().join("tools/difftest/corpus/txgen-one-node/crypto");
    let read = |name: &str| std::fs::read_to_string(corpus.join(name)).expect("the corpus's files");
    let network = Network::load(&read("network.json")).expect("the corpus's network file");
    let context = Arc::new(TxContext::new(network, read("milestones.json")).expect("a context"));
    let strategy = processor_cases(&context);
    let config = Config::default();
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &[13; 32]);
    let mut runner = TestRunner::new_with_rng(config, rng);
    let registry = subject::heartwood(None);
    let subject = registry
        .get("chain.process")
        .expect("the processor's subject");
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..cases {
        let (op, height, input) = strategy.new_tree(&mut runner).expect("a case").current();
        let case = Case {
            op,
            network: "devnet",
            height,
            input: &input,
        };
        let key = match subject.run(&case).expect("the subject answers") {
            Verdict::Output(output) => {
                for key in ["result", "unchained", "handler"] {
                    if let Some(name) = output[key].as_str() {
                        *seen.entry(format!("{key} {name}")).or_insert(0) += 1;
                    }
                }
                continue;
            }
            Verdict::RefusedByDesign { difference, .. } => format!("documented {difference:?}"),
            other => format!("refused {other:?}").chars().take(60).collect(),
        };
        *seen.entry(key).or_insert(0) += 1;
    }
    eprintln!("P13 on Heartwood's side, {cases} cases:");
    for (key, n) in &seen {
        eprintln!("  {n:>6}  {key}");
    }
    // The reach L1 asks of the reference's answers from 1,000 cases on (the rarest status comes
    // in about 1 case of 30); fewer cases need only reach each once.
    let minimum = if cases >= 1_000 { 20 } else { 1 };
    let wanted = (PROCESSOR_RESULTS.iter().map(|r| format!("result {r}")))
        .chain(UNCHAINED_STATUSES.iter().map(|u| format!("unchained {u}")));
    for key in wanted {
        let n = seen.get(&key).copied().unwrap_or(0);
        assert!(
            n >= minimum,
            "{key} reached {n} times, fewer than {minimum}"
        );
    }
}
