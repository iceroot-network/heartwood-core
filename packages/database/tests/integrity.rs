//! The boot checks pass on a store the commits wrote, and name each problem a change outside the
//! node leaves: a missing block, a count or a sum that differs, round rows above the tip's round,
//! a missing or stray undo record, and a state row that is not at the tip.

mod common;

use common::{block_id, block_row, chain, milestones, open, tamper, transaction_row};
use heartwood_database::{IntegrityProblem, IntegrityReport, StateRow};

fn problems(name: &str, tip: u32, change: &str) -> Vec<IntegrityProblem> {
    let (database, settings) = chain(name, tip);
    tamper(&settings).execute_batch(change).unwrap();
    let report = database.integrity(&milestones()).unwrap();
    assert_eq!(report.tip, Some(tip), "{change}");
    report.problems
}

#[test]
fn a_store_the_commits_wrote_passes_at_every_height() {
    let (mut database, _) = open("integrity-clean");
    let empty = database.integrity(&milestones()).unwrap();
    assert_eq!(
        empty,
        IntegrityReport {
            tip: None,
            problems: vec![]
        }
    );
    for height in 1..=12 {
        let mut chunk = database.begin().unwrap();
        common::write_block(&mut chunk, height, height % 3);
        chunk.commit().unwrap();
        let report = database.integrity(&milestones()).unwrap();
        assert_eq!(report.tip, Some(height));
        assert!(report.is_ok(), "{height}: {:?}", report.problems);
    }
}

#[test]
fn a_missing_block_is_found() {
    // Block 6 has no transactions, so only the count differs.
    assert_eq!(
        problems("integrity-block", 12, "DELETE FROM blocks WHERE height = 6"),
        [IntegrityProblem::BlockCount {
            tip: 12,
            blocks: 11
        }]
    );
}

#[test]
fn a_count_or_a_sum_that_differs_is_found() {
    assert_eq!(
        problems(
            "integrity-fee",
            12,
            "UPDATE blocks SET total_fee = total_fee + 5 WHERE height = 4"
        ),
        [IntegrityProblem::TransactionSums {
            column: "total_fee",
            blocks: 29,
            transactions: 24,
        }]
    );
    assert_eq!(
        problems(
            "integrity-amount",
            12,
            "UPDATE transactions SET amount = 0 WHERE block_height = 5 AND sequence = 1"
        ),
        [IntegrityProblem::TransactionSums {
            column: "total_amount",
            blocks: 36,
            transactions: 33,
        }]
    );
    let id = common::transaction_id(2, 0).to_hex();
    assert_eq!(
        problems(
            "integrity-count",
            12,
            &format!("DELETE FROM transactions WHERE id = '{id}'")
        ),
        [
            IntegrityProblem::TransactionSums {
                column: "number_of_transactions",
                blocks: 12,
                transactions: 11,
            },
            IntegrityProblem::TransactionSums {
                column: "total_fee",
                blocks: 24,
                transactions: 22,
            },
            IntegrityProblem::TransactionSums {
                column: "total_amount",
                blocks: 36,
                transactions: 33,
            },
        ]
    );
}

#[test]
fn sums_above_i64_are_compared_exactly() {
    let (mut database, settings) = open("integrity-large");
    let max = i64::MAX as u64;
    let mut chunk = database.begin().unwrap();
    for height in 1..=3 {
        let mut block = block_row(height, 1);
        block.total_fee = max;
        block.total_amount = max;
        let mut transaction = transaction_row(height, 0);
        transaction.fee = max;
        transaction.amount = max;
        chunk.save_block(&block, &[transaction]).unwrap();
        if height > 1 {
            chunk.put_undo(height, "").unwrap();
        }
    }
    chunk.commit().unwrap();
    assert!(database.integrity(&milestones()).unwrap().is_ok());

    tamper(&settings)
        .execute_batch("UPDATE transactions SET fee = fee - 1 WHERE block_height = 2")
        .unwrap();
    let total = 3 * u128::from(max);
    assert_eq!(
        database.integrity(&milestones()).unwrap().problems,
        [IntegrityProblem::TransactionSums {
            column: "total_fee",
            blocks: total,
            transactions: total - 1,
        }]
    );
}

#[test]
fn round_rows_above_the_tips_round_are_found() {
    // Tip 12 is in round 3 (heights 11 to 15): rows of round 4 are too early.
    assert_eq!(
        problems(
            "integrity-round",
            12,
            "INSERT INTO rounds VALUES (4, '02aa', 1)"
        ),
        [IntegrityProblem::RoundAbove {
            round: 4,
            tip_round: 3
        }]
    );
    // Tip 10 ends round 2 and saved round 3's rows: they belong to it, round 4's do not.
    assert_eq!(problems("integrity-round-end", 10, ""), []);
    assert_eq!(
        problems(
            "integrity-round-end-above",
            10,
            "INSERT INTO rounds VALUES (4, '02aa', 1)"
        ),
        [IntegrityProblem::RoundAbove {
            round: 4,
            tip_round: 3
        }]
    );
}

#[test]
fn a_missing_or_stray_undo_record_is_found() {
    assert_eq!(
        problems(
            "integrity-undo",
            12,
            "DELETE FROM undo WHERE height IN (7, 8, 11)"
        ),
        [IntegrityProblem::Undo { height: 7 }]
    );
    assert_eq!(
        problems(
            "integrity-undo-first",
            12,
            "DELETE FROM undo WHERE height = 2"
        ),
        [IntegrityProblem::Undo { height: 2 }]
    );
    assert_eq!(
        problems(
            "integrity-undo-last",
            12,
            "DELETE FROM undo WHERE height = 12"
        ),
        [IntegrityProblem::Undo { height: 12 }]
    );
    assert_eq!(
        problems(
            "integrity-undo-genesis",
            12,
            "INSERT INTO undo VALUES (1, 'genesis')"
        ),
        [IntegrityProblem::Undo { height: 1 }]
    );
    assert_eq!(
        problems(
            "integrity-undo-above",
            12,
            "INSERT INTO undo VALUES (13, 'above')"
        ),
        [IntegrityProblem::Undo { height: 13 }]
    );
}

#[test]
fn a_state_row_that_is_not_at_the_tip_is_found() {
    let id = block_id(11).to_hex();
    assert_eq!(
        problems(
            "integrity-state",
            12,
            &format!("UPDATE state SET height = 11, block_id = '{id}'")
        ),
        [IntegrityProblem::StateRow {
            height: 11,
            tip: 12
        }]
    );
    assert_eq!(
        problems(
            "integrity-state-block",
            12,
            &format!("UPDATE state SET block_id = '{id}'")
        ),
        [IntegrityProblem::StateBlock { height: 12 }]
    );
    // A missing state row is no problem: the start replays the chain.
    assert_eq!(
        problems("integrity-state-missing", 12, "DELETE FROM state"),
        []
    );

    // Rows without blocks: a store changed outside the node.
    let (mut database, _) = open("integrity-state-empty");
    let mut chunk = database.begin().unwrap();
    chunk
        .put_state(&StateRow {
            height: 3,
            block_id: block_id(3),
            record: String::new(),
        })
        .unwrap();
    chunk.save_round(&common::round_rows(1)).unwrap();
    chunk.commit().unwrap();
    assert_eq!(
        database.integrity(&milestones()).unwrap(),
        IntegrityReport {
            tip: None,
            problems: vec![
                IntegrityProblem::RoundAbove {
                    round: 1,
                    tip_round: 0
                },
                IntegrityProblem::StateRow { height: 3, tip: 0 },
            ],
        }
    );
}

#[test]
fn every_problem_is_reported_in_the_order_the_checks_run() {
    let problems = problems(
        "integrity-all",
        12,
        "DELETE FROM blocks WHERE height = 6; \
         UPDATE blocks SET total_amount = total_amount + 1 WHERE height = 1; \
         INSERT INTO rounds VALUES (9, '02aa', 1); \
         DELETE FROM undo WHERE height = 9; \
         UPDATE state SET height = 10",
    );
    assert_eq!(
        problems,
        [
            IntegrityProblem::BlockCount {
                tip: 12,
                blocks: 11
            },
            IntegrityProblem::TransactionSums {
                column: "total_amount",
                blocks: 37,
                transactions: 36,
            },
            IntegrityProblem::RoundAbove {
                round: 9,
                tip_round: 3
            },
            IntegrityProblem::Undo { height: 9 },
            IntegrityProblem::StateRow {
                height: 10,
                tip: 12
            },
        ]
    );
}
