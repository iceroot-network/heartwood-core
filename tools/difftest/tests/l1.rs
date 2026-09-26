//! L1 without an oracle: the strategies make inputs in the domain both sides accept. With a test
//! oracle configured (DIFFTEST_TEST_SOLAR_DIR or DIFFTEST_TEST_IMAGE, as `make test-oracle` sets
//! them), a short run of every property must pass.

use std::path::Path;
use std::sync::Arc;

use heartwood_crypto::Transaction;
use heartwood_crypto::utils::hex;
use heartwood_difftest::fixture::Fixture;
use heartwood_difftest::levels::l1::{self, L1Options, Property};
use heartwood_difftest::oracle::{Mode, Oracle};
use heartwood_difftest::strategies::transactions::{TxContext, fee_cases, handler_cases};
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::VectorSet;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;

fn devnet() -> (VectorSet, Devnet, Arc<TxContext>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
    let set = VectorSet::load(&dir).expect("the committed vectors load");
    let devnet = Devnet::from_vectors(&set).unwrap().expect("a genesis class");
    let text = l1::devnet_milestones(&set).expect("the devnet milestone file");
    let context = TxContext::new(devnet.network.clone(), text).expect("the milestones load");
    (set, devnet, Arc::new(context))
}

#[test]
fn handler_cases_are_well_formed() {
    let (_, devnet, context) = devnet();
    let mut runner = TestRunner::deterministic();
    let strategy = handler_cases(context);
    let mut aims = std::collections::BTreeSet::new();
    for _ in 0..300 {
        let case = strategy.new_tree(&mut runner).unwrap().current();
        let fixture = Fixture::from_json(&case.input["fixture"]).expect("the fixture parses");
        assert!(fixture.total_balance().is_some_and(|t| t <= i64::MAX as u64));
        let last = case.input["lastBlockHeight"].as_u64().unwrap();
        assert_eq!(case.height, last + 1);
        assert!((1..1_000_000).contains(&last));
        let bytes = hex::decode(case.input["hex"].as_str().unwrap()).unwrap();
        let params = devnet.milestones.at(u32::try_from(case.height).unwrap());
        let tx = Transaction::decode(&bytes, params);
        assert!(tx.is_ok() || case.input.get("milestones").is_some(), "{:?}", tx.err());
        aims.insert(case.aim.split(',').next().unwrap_or_default().to_owned());
    }
    for aim in ["transfer", "vote", "burn", "delegate registration", "revoke"] {
        assert!(aims.iter().any(|a| a.starts_with(aim)), "{aim} not reached: {aims:?}");
    }
}

#[test]
fn fee_cases_decode() {
    let (_, devnet, context) = devnet();
    let mut runner = TestRunner::deterministic();
    let strategy = fee_cases(context);
    for _ in 0..200 {
        let (height, input) = strategy.new_tree(&mut runner).unwrap().current();
        let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
        let params = devnet.milestones.at(u32::try_from(height).unwrap());
        assert!(Transaction::decode(&bytes, params).is_ok(), "{input}");
    }
}

/// A short run of every property against the configured test oracles.
#[test]
fn properties_agree_with_the_configured_oracles() {
    let mut modes = Vec::new();
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        modes.push(Mode::Local { solar_dir: dir.into() });
    }
    if let Ok(image) = std::env::var("DIFFTEST_TEST_IMAGE") {
        modes.push(Mode::Container { image });
    }
    if modes.is_empty() {
        eprintln!("properties_agree_with_the_configured_oracles: no test oracle configured");
        return;
    }
    let (_, devnet, context) = devnet();
    let registry = subject::heartwood(Some(devnet));
    let cases = std::env::var("DIFFTEST_TEST_L1_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(256);
    for mode in modes {
        let mut server = Oracle::new(mode.clone()).server().unwrap();
        let run = l1::run(
            &mut server,
            &registry,
            &context,
            &L1Options {
                properties: Property::ALL.to_vec(),
                cases,
                seed: 1,
                regressions: None,
            },
        )
        .unwrap();
        assert!(run.report.is_success(), "{mode:?}\n{}", run.report);
    }
}
