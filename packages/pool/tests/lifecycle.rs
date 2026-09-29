//! The pool after admission: per-sender views, removals with their cascades, forged removal,
//! clean-up, re-adds, a restart from `pool.sqlite`, and the storage written only after
//! admission.

mod support;

use heartwood_crypto::Transaction;
use heartwood_pool::{Checked, Pool, Readded};
use heartwood_state::wallets::WalletRepository;
use support::*;

/// Alice's transfers with nonces 2, 3 and 4.
fn chain(fixture: &Fixture) -> Vec<Transaction> {
    (2..=4)
        .map(|nonce| fixture.alice.transfer(&fixture.bob, 1, nonce, FEE))
        .collect()
}

#[test]
fn a_pending_incoming_transfer_does_not_fund_its_recipient() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("views", |_| {});
    let to_dave = fixture.bob.transfer(&fixture.dave, 50 * FEE, 1, FEE);
    let from_dave = fixture.dave.transfer(&fixture.bob, 1, 1, FEE);
    let result = harness.submit(&ctx, &[&to_dave, &from_dave]);
    assert_eq!(result.accept, [id(&to_dave)]);
    assert_eq!(error_of(&result, &id(&from_dave)).0, "ERR_APPLY");
    // Each sender's view has its own transactions applied, and only them.
    let bob = harness.pool.pool_wallet(&fixture.bob.address).unwrap();
    assert_eq!((bob.balance(), bob.nonce()), (1_000 * FEE - 51 * FEE, 1));
    assert!(harness.pool.pool_wallet(&fixture.dave.address).is_none());
    // The state is not changed.
    assert_eq!(
        fixture
            .wallets
            .find_by_address(&fixture.bob.address)
            .unwrap()
            .nonce(),
        0
    );
}

#[test]
fn removal_reverts_the_later_transactions_newest_first() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("remove", |_| {});
    let txs = chain(&fixture);
    let other = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    harness.submit(&ctx, &[&txs[0], &txs[1], &txs[2], &other]);
    let removed = harness
        .pool
        .remove_transaction(&ctx, &fixture.alice.address, txs[1].id())
        .unwrap();
    assert_eq!(shared_ids(&removed), [id(&txs[2]), id(&txs[1])]);
    assert_eq!(
        harness
            .pool
            .pool_wallet(&fixture.alice.address)
            .unwrap()
            .nonce(),
        2
    );
    assert_eq!(harness.stored(), [id(&txs[0]), id(&other)]);
    // The sender may go on from the reverted nonce.
    let next = fixture.alice.transfer(&fixture.carol, 7, 3, FEE);
    assert_eq!(harness.submit(&ctx, &[&next]).accept, [id(&next)]);
    // A transaction that is not stored is not looked for.
    let removed = harness
        .pool
        .remove_transaction(&ctx, &fixture.alice.address, txs[1].id())
        .unwrap();
    assert!(removed.is_empty());
    // Removing a sender's first transaction drops its sender mempool.
    let removed = harness
        .pool
        .remove_transaction(&ctx, &fixture.alice.address, txs[0].id())
        .unwrap();
    assert_eq!(shared_ids(&removed), [id(&next), id(&txs[0])]);
    assert!(
        harness
            .pool
            .mempool()
            .sender(&fixture.alice.address)
            .is_none()
    );
    assert_eq!(harness.all(), [id(&other)]);
}

#[test]
fn a_forged_transaction_takes_its_earlier_ones_without_a_revert() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("forged", |_| {});
    let txs = chain(&fixture);
    harness.submit(&ctx, &[&txs[0], &txs[1], &txs[2]]);
    let removed = harness
        .pool
        .remove_forged_transaction(&fixture.alice.address, txs[1].id())
        .unwrap();
    assert_eq!(shared_ids(&removed), [id(&txs[0]), id(&txs[1])]);
    assert_eq!(harness.all(), [id(&txs[2])]);
    assert_eq!(harness.stored(), [id(&txs[2])]);
    // The view keeps every applied transaction.
    assert_eq!(
        harness
            .pool
            .pool_wallet(&fixture.alice.address)
            .unwrap()
            .nonce(),
        4
    );
    // Not stored: nothing happens.
    let removed = harness
        .pool
        .remove_forged_transaction(&fixture.alice.address, txs[0].id())
        .unwrap();
    assert!(removed.is_empty());
}

#[test]
fn a_forged_transaction_only_in_the_storage_loses_its_row() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("forged-row", |_| {});
    let txs = chain(&fixture);
    harness.submit(&ctx, &[&txs[0]]);
    // After a restart, before the re-add: the row is there, the mempool is empty.
    let options = harness.pool.options().clone();
    drop(harness.pool);
    let mut pool = Pool::open(options, None).unwrap();
    let removed = pool
        .remove_forged_transaction(&fixture.alice.address, txs[0].id())
        .unwrap();
    assert!(removed.is_empty());
    assert!(pool.stored().unwrap().is_empty());
}

#[test]
fn clean_up_removes_old_then_lowest_priority() {
    let fixture = Fixture::new();
    let milestones = &*MILESTONES;
    let mut harness = Harness::new("clean-up", |options| {
        options.max_transaction_age = 10;
        options.max_transactions_in_pool = 5;
    });
    let txs = chain(&fixture);
    let at = |tip| {
        let mut ctx = fixture.ctx(milestones);
        ctx.tip = tip;
        ctx
    };
    harness.submit(&at(490), &[&txs[0], &txs[1]]);
    let later = fixture.carol.transfer(&fixture.bob, 1, 4, 2 * FEE);
    let lowest = fixture.bob.transfer(&fixture.carol, 1, 1, FEE);
    harness.submit(&at(495), &[&later, &lowest, &txs[2]]);
    // At tip 499 nothing is old (490 > 499 − 10).
    let removed = harness.pool.clean_up(&at(499)).unwrap();
    assert!(removed.old.is_empty() && removed.lowest_priority.is_empty());
    // At tip 500, rows at height 490 are old: the newest first, each with the sender's later
    // ones (alice's third transaction, stored at 495, goes with them).
    let removed = harness.pool.clean_up(&at(500)).unwrap();
    assert_eq!(
        shared_ids(&removed.old),
        [id(&txs[2]), id(&txs[1]), id(&txs[0])]
    );
    assert!(removed.lowest_priority.is_empty());
    assert_eq!(harness.all(), [id(&later), id(&lowest)]);
    assert_eq!(harness.stored(), [id(&later), id(&lowest)]);
    // A lower limit (a new setting) removes the lowest priority.
    let options = {
        let mut options = harness.pool.options().clone();
        options.max_transactions_in_pool = 1;
        options
    };
    drop(harness.pool);
    let mut pool = Pool::open(options, None).unwrap();
    let readded = pool.readd(&at(500), &[]).unwrap();
    assert_eq!(readded.stored.len(), 1, "the re-add made room: {readded:?}");
    assert_eq!(ids(pool.query().all()), [id(&later)]);
    assert!(pool.clean_up(&at(500)).unwrap().lowest_priority.is_empty());
}

#[test]
fn readd_restores_the_pool_from_its_storage() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("restart", |_| {});
    let txs = chain(&fixture);
    let other = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    harness.submit(&ctx, &[&txs[0], &other, &txs[1]]);
    let before = harness.all();
    let options = harness.pool.options().clone();
    drop(harness.pool);
    let mut pool = Pool::open(options, None).unwrap();
    assert_eq!(pool.size(), 0);
    let readded = pool.readd(&ctx, &[]).unwrap();
    assert_eq!(
        readded,
        Readded {
            stored: vec![*txs[0].id(), *other.id(), *txs[1].id()],
            ..Readded::default()
        }
    );
    assert_eq!(ids(pool.query().all()), before);
    // A re-add after a flush of the mempool gives the same pool.
    let again = pool.readd(&ctx, &[]).unwrap();
    assert_eq!(again.stored.len(), 3);
    assert_eq!(ids(pool.query().all()), before);
}

#[test]
fn readd_drops_what_no_longer_applies_and_what_is_too_old() {
    let mut fixture = Fixture::new();
    let mut harness = Harness::new("readd-drop", |options| options.max_transaction_age = 10);
    let txs = chain(&fixture);
    let other = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    {
        let mut ctx = fixture.ctx(&MILESTONES);
        ctx.tip = 495;
        harness.submit(&ctx, &[&other]);
        ctx.tip = 500;
        harness.submit(&ctx, &[&txs[0], &txs[1]]);
    }
    // A block forged alice's second transaction elsewhere: her nonce is now 3.
    fixture
        .wallets
        .get_or_create(&fixture.alice.address)
        .set_nonce(3);
    let mut ctx = fixture.ctx(&MILESTONES);
    ctx.tip = 505;
    let readded = harness.pool.readd(&ctx, &[]).unwrap();
    assert_eq!(readded.expired, [*other.id()]);
    assert_eq!(
        readded.stored,
        Vec::<heartwood_crypto::TransactionId>::new()
    );
    assert_eq!(
        readded.failed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [*txs[0].id(), *txs[1].id()]
    );
    assert!(
        readded.failed[0]
            .1
            .contains("Cannot apply a transaction with nonce 2")
    );
    assert_eq!(harness.pool.size(), 0);
    assert!(harness.stored().is_empty());
}

#[test]
fn readd_puts_back_previously_forged_transactions_first() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("readd-forged", |_| {});
    let txs = chain(&fixture);
    let other = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    harness.submit(&ctx, &[&other, &txs[0], &txs[1]]);
    // Alice's first transaction is forged, then its block is reverted.
    harness
        .pool
        .remove_forged_transaction(&fixture.alice.address, txs[0].id())
        .unwrap();
    let readded = harness.pool.readd(&ctx, &[txs[0].clone()]).unwrap();
    assert_eq!(readded.forged, [*txs[0].id()]);
    assert_eq!(readded.stored, [*other.id(), *txs[1].id()]);
    // The forged one's sender mempool comes first, and its row last.
    assert_eq!(harness.all(), [id(&txs[1]), id(&txs[0]), id(&other)]);
    assert_eq!(harness.stored(), [id(&other), id(&txs[1]), id(&txs[0])]);
    // A previously forged transaction that is still stored counts as failed but stays in the
    // mempool; its row is re-added like the others and fails on its nonce.
    let readded = harness.pool.readd(&ctx, &[txs[0].clone()]).unwrap();
    assert!(readded.forged.is_empty());
    assert_eq!(
        readded.forged_failed,
        [(
            *txs[0].id(),
            "UNIQUE constraint failed: pool_20220908.id".to_owned()
        )]
    );
    assert_eq!(readded.stored, [*other.id(), *txs[1].id()]);
    assert_eq!(
        readded.failed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [*txs[0].id()]
    );
    assert_eq!(harness.all(), [id(&txs[1]), id(&txs[0]), id(&other)]);
    assert_eq!(harness.stored(), [id(&other), id(&txs[1])]);
}

#[test]
fn flush_empties_the_mempool_and_the_storage() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("flush", |_| {});
    harness.submit(&ctx, &[&chain(&fixture)[0]]);
    harness.pool.flush().unwrap();
    assert_eq!(harness.pool.size(), 0);
    assert!(harness.stored().is_empty());
}

#[test]
fn the_clear_marker_empties_the_pool_at_open() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("marker", |_| {});
    harness.submit(&ctx, &[&chain(&fixture)[0]]);
    let options = harness.pool.options().clone();
    let marker = harness.dir.join("clear-pool.lock");
    drop(harness.pool);
    std::fs::write(&marker, b"").unwrap();
    let mut pool = Pool::open(options, Some(&marker)).unwrap();
    assert!(pool.stored().unwrap().is_empty());
    assert!(pool.readd(&ctx, &[]).unwrap().stored.is_empty());
    assert!(!marker.exists());
}

/// The number of rows the pool's table has ever had: its `AUTOINCREMENT` counter.
fn rows_ever_written(pool: &Pool) -> i64 {
    let path = &pool.options().storage;
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name = 'pool_20220908'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

#[test]
fn only_admitted_transactions_are_ever_written() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("written", |options| options.max_transactions_in_pool = 2);
    let txs = chain(&fixture);
    let refused = [
        fixture.dave.transfer(&fixture.bob, 1, 1, FEE),
        fixture.alice.transfer(&fixture.bob, 1, 9, FEE),
        fixture.carol.transfer(&fixture.bob, 1, 4, 1),
    ];
    let result = harness.submit(
        &ctx,
        &[&refused[0], &txs[0], &refused[1], &txs[1], &refused[2]],
    );
    assert_eq!(result.accept.len(), 2);
    assert_eq!(result.invalid.len(), 3);
    // The reference writes a row for every entry that reaches the second loop and deletes the
    // refused ones; here a refused transaction never had a row, so a crash at any point of its
    // checks leaves none for the re-add to trust.
    assert_eq!(rows_ever_written(&harness.pool), 2);
    let full = fixture.bob.transfer(&fixture.carol, 1, 1, FEE);
    assert_eq!(
        error_of(&harness.submit(&ctx, &[&full]), &id(&full)).0,
        "ERR_POOL_FULL"
    );
    assert_eq!(rows_ever_written(&harness.pool), 2);
}

/// The options of the crash test: no practical per-sender limit.
fn crash_options(dir: &std::path::Path) -> heartwood_pool::Options {
    let mut options = options(dir);
    options.max_transactions_per_sender = 100_000;
    options
}

/// The child of [`a_crash_leaves_no_row_of_an_unadmitted_transaction`]: submits refused and
/// admitted transactions one at a time until it is killed.
#[test]
#[ignore = "run by the crash test in a child process"]
fn crash_child() {
    let Ok(dir) = std::env::var("HEARTWOOD_POOL_CRASH_DIR") else {
        return;
    };
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut pool = Pool::open(crash_options(std::path::Path::new(&dir)), None).unwrap();
    let mut processor = heartwood_pool::Processor::new();
    let clock = heartwood_kernel::clock::ManualClock::new(NOW_MS);
    for nonce in 2..u64::MAX {
        let refused = fixture.dave.transfer(&fixture.bob, nonce, 1, FEE);
        let admitted = fixture.alice.transfer(&fixture.bob, 1, nonce, FEE);
        for tx in [&refused, &admitted] {
            processor
                .process(&mut pool, &ctx, &clock, vec![bytes(tx)])
                .unwrap();
        }
    }
}

#[test]
fn a_crash_leaves_no_row_of_an_unadmitted_transaction() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let dave = fixture.dave.address.to_base58();
    let exe = std::env::current_exe().unwrap();
    for round in 0..6u64 {
        let dir = tempdir("crash");
        let mut child = std::process::Command::new(&exe)
            .args(["--exact", "crash_child", "--ignored", "--nocapture"])
            .env("HEARTWOOD_POOL_CRASH_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300 + 97 * round));
        child.kill().unwrap();
        child.wait().unwrap();
        let mut pool = Pool::open(crash_options(&dir), None).unwrap();
        let stored = pool.stored().unwrap();
        assert!(
            !stored.is_empty(),
            "the child admitted nothing before it was killed"
        );
        assert!(
            stored.iter().all(|row| row.sender_id != dave),
            "an unadmitted row"
        );
        // Every row the re-add trusts is a transaction that was admitted.
        let readded = pool.readd(&ctx, &[]).unwrap();
        assert!(readded.failed.is_empty(), "{readded:?}");
        assert_eq!(readded.stored.len(), stored.len());
    }
}

#[test]
fn threads_decode_as_one_thread_does() {
    let fixture = Fixture::new();
    let txs = chain(&fixture);
    let mut entries = Vec::new();
    for (n, tx) in txs.iter().enumerate() {
        entries.push(bytes(tx));
        entries.push(heartwood_pool::Entry::Data(tx.to_json(params())));
        entries.push(heartwood_pool::Entry::Bytes(tx.bytes()[..n * 20].to_vec()));
        let mut removed = tx.bytes().to_vec();
        removed[7] = 8;
        entries.push(heartwood_pool::Entry::Bytes(removed));
    }
    let one = heartwood_pool::verifier::check_entries(&entries, params(), 1);
    for workers in [2, 3, 5, 64] {
        assert_eq!(
            heartwood_pool::verifier::check_entries(&entries, params(), workers),
            one
        );
    }
    assert!(matches!(one[0], Ok(Checked::Decoded(_))));
    assert!(one[2].is_err());
    assert!(matches!(one[3], Ok(Checked::Removed { kind: 8, .. })));
    assert!(heartwood_pool::verifier::check_entries(&[], params(), 3).is_empty());
}

#[test]
fn the_view_starts_as_the_state() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("view", |_| {});
    let txs = chain(&fixture);
    harness.submit(&ctx, &[&txs[0]]);
    let view: &WalletRepository = harness
        .pool
        .mempool()
        .sender(&fixture.alice.address)
        .unwrap()
        .state()
        .wallets();
    assert_eq!(view.len(), fixture.wallets.len());
    assert_eq!(
        view.find_by_address(&fixture.alice.address)
            .unwrap()
            .nonce(),
        2
    );
}

#[test]
fn a_failed_revert_leaves_a_view_that_asks_for_a_retry() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let options = options(&tempdir("retry"));
    let mempool = heartwood_pool::mempool::Mempool::new();
    let pending = heartwood_pool::Query::new(&mempool);
    let mut state = heartwood_pool::sender_state::SenderState::new(&fixture.wallets);
    let txs = chain(&fixture);
    let undo = state.apply(&ctx, &options, &pending, &txs[0]).unwrap();
    // Reverting a transaction that is not the last one applied fails, and marks the view.
    assert!(state.revert(&ctx, &txs[1], &undo).is_err());
    assert!(state.is_corrupt());
    let refusal = state.apply(&ctx, &options, &pending, &txs[1]).unwrap_err();
    assert_eq!(
        (refusal.code, refusal.message),
        (
            heartwood_pool::PoolCode::Retry,
            format!(
                "{} cannot be added to the pool, please retry",
                text(&txs[1])
            )
        )
    );
}
