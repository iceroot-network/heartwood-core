//! L1 without an oracle: the strategies make inputs in the domain both sides accept, and the
//! minimal cases that long runs saved are answered as the reference answered them. With a test
//! oracle configured (DIFFTEST_TEST_SOLAR_DIR or DIFFTEST_TEST_IMAGE, as `make test-oracle` sets
//! them), a short run of every property must pass.

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::utils::hex;
use heartwood_difftest::fixture::Fixture;
use heartwood_difftest::levels;
use heartwood_difftest::levels::l1::{self, L1Options, Property};
use heartwood_difftest::oracle::{Mode, Oracle};
use heartwood_difftest::report::Outcome;
use heartwood_difftest::strategies::transactions::{TxContext, fee_cases, handler_cases};
use heartwood_difftest::subject;
use heartwood_difftest::subjects::Devnet;
use heartwood_difftest::vectors::{Expected, VectorSet};
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use serde_json::Value;

fn devnet() -> (VectorSet, Devnet, Arc<TxContext>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/crypto/tests/vectors");
    let set = VectorSet::load(&dir).expect("the committed vectors load");
    let devnet = Devnet::from_vectors(&set)
        .unwrap()
        .expect("a genesis class");
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
        assert!(
            fixture
                .total_balance()
                .is_some_and(|t| t <= i64::MAX as u64)
        );
        let last = case.input["lastBlockHeight"].as_u64().unwrap();
        assert_eq!(case.height, last + 1);
        assert!((1..1_000_000).contains(&last));
        let bytes = hex::decode(case.input["hex"].as_str().unwrap()).unwrap();
        let params = devnet.milestones.at(u32::try_from(case.height).unwrap());
        let tx = Transaction::decode(&bytes, params);
        assert!(
            tx.is_ok() || case.input.get("milestones").is_some(),
            "{:?}",
            tx.err()
        );
        aims.insert(case.aim.split(',').next().unwrap_or_default().to_owned());
    }
    for aim in [
        "transfer",
        "vote",
        "burn",
        "delegate registration",
        "revoke",
    ] {
        assert!(
            aims.iter().any(|a| a.starts_with(aim)),
            "{aim} not reached: {aims:?}"
        );
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

/// P1 makes a case of almost every draw: a draw the builder refuses is thrown away, and a run
/// stops once proptest's limit of such draws is reached. Its votes reach 20 names and more.
#[test]
fn fee_cases_are_made_from_almost_every_draw() {
    let (_, devnet, context) = devnet();
    let config = Config {
        cases: 1_000,
        max_local_rejects: 10,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::deterministic_rng(RngAlgorithm::ChaCha));
    let most_votes = Cell::new(0);
    let outcome = runner.run(&fee_cases(context), |(height, input)| {
        let bytes = hex::decode(input["hex"].as_str().unwrap()).unwrap();
        let params = devnet.milestones.at(u32::try_from(height).unwrap());
        let tx = Transaction::decode(&bytes, params).unwrap();
        if let Asset::Vote { votes } = &tx.data().asset {
            most_votes.set(most_votes.get().max(votes.len()));
        }
        Ok(())
    });
    assert!(outcome.is_ok(), "{outcome:?}\n{runner}");
    assert!(most_votes.get() >= 20, "{}", most_votes.get());
}

/// The minimal cases that long runs of the properties saved (`tests/fixtures/l1-saved`, one JSON
/// line per case: the operation, the network, the height, the input and the reference's answer as
/// its oracle gave it) are judged as L1 judges a case, without an oracle: what a long run found
/// stays found on every test run.
#[test]
fn saved_minimal_cases_are_answered_as_the_reference_answered_them() {
    let (_, devnet, _) = devnet();
    let registry = subject::heartwood(Some(devnet));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/l1-saved");
    let mut judged = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for (at, line) in text.lines().enumerate() {
            let record: Value = serde_json::from_str(line).unwrap();
            let case = subject::Case {
                op: record["op"].as_str().unwrap(),
                network: record["network"].as_str().unwrap(),
                height: record["height"].as_u64().unwrap(),
                input: &record["input"],
            };
            let expected = Expected::Output(record["output"].clone());
            let outcome = levels::judge_documented(&case, &expected, &registry);
            let place = format!("{}:{}", path.display(), at + 1);
            // Compared and equal, or a documented difference: a skipped case, or one with no
            // subject, checks nothing.
            let answered = matches!(outcome, Outcome::Pass | Outcome::Documented(_));
            assert!(answered, "{place}: {outcome:?}");
            judged += 1;
        }
    }
    assert!(judged > 0, "no saved case in {}", dir.display());
}

/// A short run of every property against the configured test oracles.
#[test]
fn properties_agree_with_the_configured_oracles() {
    let mut modes = Vec::new();
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        modes.push(Mode::Local {
            solar_dir: dir.into(),
        });
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
                min_reach: 0,
                decision_reach: 0,
                served_reach: 0,
            },
        )
        .unwrap();
        assert!(run.report.is_success(), "{mode:?}\n{}", run.report);
    }
}
