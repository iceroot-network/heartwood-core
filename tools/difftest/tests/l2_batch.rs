//! The lockstep batch and L2 over it, against the real reference (the integration tests, ignored
//! for the reason `integration`, which `make test-integration` runs).
//!
//! A batch of drawn chains is made in parallel by the producer, each chain judged by the reference
//! and replayed in Heartwood (the store, both restart paths, both job cuts, the revert symmetry),
//! and a long chain shows the expiry of an online entry in both pools. Without an oracle the
//! integration tests fail, unless `DIFFTEST_TEST_NO_ORACLE` says to skip them, as `lockstep.rs`'s
//! do. The tests that need no oracle run in every `cargo test`.

use std::path::{Path, PathBuf};

use heartwood_difftest::cli::{L2BatchArgs, LockstepBatchArgs, OracleArgs};
use heartwood_difftest::errors::Error;
use heartwood_difftest::levels::l2_batch;
use heartwood_difftest::lockstep::batch::{self, BatchChain};
use heartwood_difftest::lockstep::{self, Attested, LockstepError, Step};
use heartwood_difftest::report::Outcome;
use heartwood_difftest::store::ScratchDir;

/// The reference the integration tests run against, as flags for the harness.
fn oracle_args() -> Option<OracleArgs> {
    if let Ok(dir) = std::env::var("DIFFTEST_TEST_SOLAR_DIR") {
        return Some(OracleArgs {
            solar_dir: Some(PathBuf::from(dir)),
            ..OracleArgs::default()
        });
    }
    std::env::var("DIFFTEST_TEST_IMAGE")
        .ok()
        .map(|image| OracleArgs {
            image: Some(image),
            ..OracleArgs::default()
        })
}

/// The oracle of the integration test `test`; `None` when the run skips it on purpose
/// (`DIFFTEST_TEST_NO_ORACLE`). Without either, the test fails: it must not pass unrun.
fn oracle_or_skip(test: &str) -> Option<OracleArgs> {
    if let Some(oracle) = oracle_args() {
        return Some(oracle);
    }
    assert!(
        std::env::var_os("DIFFTEST_TEST_NO_ORACLE").is_some(),
        "{test}: no test oracle configured: set DIFFTEST_TEST_IMAGE or DIFFTEST_TEST_SOLAR_DIR \
         (make test-integration sets the image when it is built), or DIFFTEST_TEST_NO_ORACLE to \
         skip the test"
    );
    eprintln!("{test}: skipped (DIFFTEST_TEST_NO_ORACLE)");
    None
}

fn batch_args(
    oracle: OracleArgs,
    out: &Path,
    seed: &str,
    count: u32,
    rounds: u32,
) -> LockstepBatchArgs {
    LockstepBatchArgs {
        oracle,
        count,
        rounds,
        seed: seed.into(),
        long: rounds > 5,
        profile: "devnet".into(),
        out: out.to_path_buf(),
        jobs: Some(count.max(1) as usize),
    }
}

fn judge_args(oracle: OracleArgs, out: &Path, seed: &str) -> L2BatchArgs {
    L2BatchArgs {
        oracle,
        dir: out.to_path_buf(),
        batches: vec![seed.into()],
    }
}

/// The cases of `report` whose operation is `op`.
fn cases<'a>(
    report: &'a heartwood_difftest::report::Report,
    op: &'a str,
) -> impl Iterator<Item = &'a heartwood_difftest::report::CaseResult> {
    report.results.iter().filter(move |r| r.op == op)
}

#[test]
fn the_batch_commands_refuse_what_a_batch_cannot_be_before_any_oracle_starts() {
    let scratch = ScratchDir::new("l2-batch-refusals").expect("scratch");
    let out = scratch.path().join("corpus");
    let none = OracleArgs::default();
    let args = |count, rounds, long| LockstepBatchArgs {
        long,
        ..batch_args(none.clone(), &out, "refusals", count, rounds)
    };
    for (count, rounds, long) in [(0, 2, false), (1, 6, false), (1, 6, true), (1, 51, true)] {
        let refused = batch::run(&args(count, rounds, long)).expect_err("a usage error");
        assert!(matches!(refused, Error::Usage(_)), "{refused}");
    }
    assert!(!out.join("batch-refusals.json").exists());
    // A directory without a batch file has nothing to judge.
    std::fs::create_dir_all(&out).expect("dir");
    let refused = l2_batch::run(&judge_args(none, &out, "refusals")).expect_err("no batch");
    assert!(matches!(refused, Error::Usage(m) if m.contains("no batch refusals")));
}

#[test]
#[ignore = "integration"]
fn a_batch_of_drawn_chains_goes_through_the_judge_and_every_replay() {
    let Some(oracle) = oracle_or_skip("the lockstep batch") else {
        return;
    };
    let scratch = ScratchDir::new("l2-batch-live").expect("scratch");
    let out = scratch.path().join("corpus");
    let made = batch::run(&batch_args(oracle.clone(), &out, "live", 4, 3)).expect("the batch");
    assert_eq!(made.chains.len(), 4);
    for chain in &made.chains {
        assert_eq!(chain.stopped, None, "{}", chain.drawn.name);
        assert_eq!(chain.tip, Some(159), "{}", chain.drawn.name);
        assert!(chain.transactions > 10, "{}", chain.drawn.name);
        assert!(out.join(&chain.drawn.name).join("corpus.json").exists());
    }
    assert_eq!(
        batch::read_batch(&batch::batch_file(&out, "live")).expect("the file"),
        made
    );
    let report = l2_batch::run(&judge_args(oracle, &out, "live")).expect("the judged batch");
    assert!(report.passes(true), "{report}");
    // Every chain went through the judge, the restart paths, the processor and the checks.
    for chain in &made.chains {
        let name = &chain.drawn.name;
        let mine = |r: &&heartwood_difftest::report::CaseResult| r.file.starts_with(name.as_str());
        assert!(
            cases(&report, "judge").filter(mine).count() >= 1,
            "{name}: judge"
        );
        assert!(
            cases(&report, "restart").filter(mine).count() >= 1,
            "{name}: restart points"
        );
        assert!(
            report
                .results
                .iter()
                .filter(mine)
                .any(|r| r.file.ends_with("ts-digests.jsonl")),
            "{name}: l2"
        );
        assert!(
            report
                .results
                .iter()
                .filter(mine)
                .any(|r| r.file.contains("(jobs: reference)"))
                && report
                    .results
                    .iter()
                    .filter(mine)
                    .any(|r| r.file.contains("(jobs: 1)")),
            "{name}: processor, in both job cuts"
        );
        assert!(
            cases(&report, "revert").filter(mine).count() >= 2,
            "{name}: reverts"
        );
        assert!(
            report
                .results
                .iter()
                .filter(mine)
                .all(|r| !r.outcome.is_failure()),
            "{name}"
        );
    }
}

/// The online steps of round `round` of `chain` as the names they leave out, or give.
fn attested(chain: &BatchChain, round: u64) -> Option<&Attested> {
    chain.drawn.recipe.steps.iter().find_map(|s| match s {
        Step::Online {
            round: r,
            delegates,
        } if *r == round => Some(delegates),
        _ => None,
    })
}

#[test]
#[ignore = "integration"]
fn a_long_chain_shows_the_online_entry_expire_in_both_pools() {
    let Some(oracle) = oracle_or_skip("the long chain") else {
        return;
    };
    let scratch = ScratchDir::new("l2-batch-long").expect("scratch");
    let out = scratch.path().join("corpus");
    // Seven rounds, the fewest that show it: the entry of round 1 is forgotten at round 7's start.
    let made = batch::run(&batch_args(oracle.clone(), &out, "long", 1, 7)).expect("the batch");
    let chain = &made.chains[0];
    assert_eq!(
        chain.stopped, None,
        "the producer stopped: both pools must agree on every vote"
    );
    assert_eq!(chain.tip, Some(371));
    // The vote the recipe expects refused at round 7's first block was refused by both pools (the
    // producer stops when either admits it), after the vote of round 6 that both admitted.
    let refused: Vec<u32> = chain
        .drawn
        .recipe
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Transactions { height, build } => Some((height, build)),
            _ => None,
        })
        .flat_map(|(h, build)| {
            build
                .iter()
                .filter(|b| {
                    b.asset.to_string().contains("genesis_50")
                        && b.refused.as_deref() == Some("ERR_OFFLINE")
                })
                .map(move |_| *h)
        })
        .collect();
    assert_eq!(refused, vec![319]);
    let delegates = attested(chain, 7).expect("an online step of round 7");
    assert!(
        matches!(delegates, Attested::AllBut(names) if names.iter().any(|n| n == "genesis_50"))
            || matches!(delegates, Attested::Only(names) if !names.iter().any(|n| n == "genesis_50"))
    );
    // The same recipe with the delegate attested again in round 2 onward: its vote at round 7 is
    // admitted, so the pools do not refuse it, and the producer stops on the recipe's expectation.
    let mut control = chain.drawn.recipe.clone();
    for step in &mut control.steps {
        if let Step::Online { round, delegates } = step
            && *round > 1
        {
            *delegates = match delegates.clone() {
                Attested::AllBut(names) => {
                    Attested::AllBut(names.into_iter().filter(|n| n != "genesis_50").collect())
                }
                Attested::Only(mut names) => {
                    names.push("genesis_50".into());
                    Attested::Only(names)
                }
                all => all,
            };
        }
    }
    let profile = scratch.path().join("control");
    let stopped = lockstep::make(&chain.drawn.seed, "devnet", 7, &control, &profile, &oracle)
        .expect_err("an attested delegate's vote is not refused");
    assert!(
        matches!(&stopped, LockstepError::Pool { detail, .. } if detail.contains("ERR_OFFLINE")),
        "{stopped}"
    );
    // The long chain judges and replays as any other.
    let report = l2_batch::run(&judge_args(oracle, &out, "long")).expect("the judged batch");
    assert!(report.passes(true), "{report}");
    assert!(
        report
            .results
            .iter()
            .all(|r| !matches!(r.outcome, Outcome::HarnessError(_))),
        "{report}"
    );
}
