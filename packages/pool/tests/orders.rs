//! The pool's orders: the query's four orders with their ties, and the collation of a block
//! candidate (fee order, the block's limits read at the tip, the stop on the byte budget, a
//! failed sender skipped, `exclude`, `validate`).

mod support;

use std::sync::Arc;

use heartwood_crypto::Transaction;
use heartwood_pool::collate;
use heartwood_transactions::PoolQuery;
use support::*;

/// Alice: nonce 2 at 3 fees, nonce 3 at 1; bob: nonce 1 at 2, nonce 2 at 1; carol: nonce 4 at
/// 1. Returned in that order.
fn populate(fixture: &Fixture, harness: &mut Harness) -> Vec<Transaction> {
    let ctx = fixture.ctx(&MILESTONES);
    let txs = vec![
        fixture.alice.transfer(&fixture.bob, 1, 2, 3 * FEE),
        fixture.alice.transfer(&fixture.bob, 1, 3, FEE),
        fixture.bob.transfer(&fixture.carol, 1, 1, 2 * FEE),
        fixture.bob.transfer(&fixture.carol, 1, 2, FEE),
        fixture.carol.transfer(&fixture.bob, 1, 4, FEE),
    ];
    let refs: Vec<&Transaction> = txs.iter().collect();
    assert_eq!(harness.submit(&ctx, &refs).accept.len(), 5);
    txs
}

#[test]
fn the_query_orders_and_their_ties() {
    let fixture = Fixture::new();
    let mut harness = Harness::new("query", |_| {});
    let txs = populate(&fixture, &mut harness);
    let [a2, a3, b1, b2, c4] = [0, 1, 2, 3, 4].map(|n| id(&txs[n]));
    let query = harness.pool.query();
    // Senders in creation order, each newest first.
    assert_eq!(ids(query.all()), [a3.as_str(), &a2, &b2, &b1, &c4]);
    assert_eq!(
        ids(query.by_sender(&fixture.bob.address)),
        [b1.as_str(), &b2]
    );
    assert!(query.by_sender(&fixture.dave.address).is_empty());
    // Each sender's newest-first list merged by fee, lowest first; a tie goes to the earliest
    // sender mempool: this is not a sort of the pool by fee.
    assert_eq!(
        ids(query.from_lowest_priority()),
        [a3.as_str(), &b2, &c4, &b1, &a2]
    );
    assert_eq!(query.lowest_priority().map(id), Some(a3.clone()));
    // Each sender's oldest-first list merged by fee, highest first.
    assert_eq!(
        ids(query.from_highest_priority()),
        [a2.as_str(), &b1, &a3, &b2, &c4]
    );
    assert!(query.has_sender_kind(
        &fixture.alice.address,
        heartwood_crypto::TransactionType::Transfer
    ));
    assert!(!query.has_sender_kind(
        &fixture.alice.address,
        heartwood_crypto::TransactionType::Vote
    ));
    assert!(!query.has_pending_username("nobody"));
}

#[test]
fn a_view_lists_the_highest_priority_first_and_copies_no_transaction() {
    let fixture = Fixture::new();
    let mut harness = Harness::new("view", |_| {});
    let txs = populate(&fixture, &mut harness);
    let online = heartwood_pool::DelegateVersions::new();
    let view = harness.pool.view(&online);
    let highest = ids(harness.pool.query().from_highest_priority());
    assert_eq!(shared_ids(&view.by_priority), highest);
    // Two views hold the same transactions, the pool's own; so does one after the other.
    let again = harness.pool.view(&online);
    let after = harness.pool.view_after(&online, &view);
    for other in [&again, &after] {
        let pairs = view.by_priority.iter().zip(&other.by_priority);
        assert!(pairs.clone().count() == 5 && pairs.into_iter().all(|(a, b)| Arc::ptr_eq(a, b)));
    }
    // A removal leaves the other transactions shared with the views before it.
    let ctx = fixture.ctx(&MILESTONES);
    let removed = harness
        .pool
        .remove_transaction(&ctx, &fixture.carol.address, txs[4].id())
        .unwrap();
    assert!(Arc::ptr_eq(&removed[0], view.get(txs[4].id()).unwrap()));
    let later = harness.pool.view_after(&online, &view);
    assert_eq!(later.len(), 4);
    assert!((later.by_priority.iter()).all(|t| Arc::ptr_eq(t, view.get(t.id()).unwrap())));
}

#[test]
fn an_empty_pool_has_no_lowest() {
    let harness = Harness::new("empty", |_| {});
    let query = harness.pool.query();
    assert!(query.all().is_empty());
    assert!(query.from_lowest_priority().is_empty());
    assert_eq!(query.lowest_priority(), None);
}

#[test]
fn collation_takes_the_highest_priority_first() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("collate", |_| {});
    let txs = populate(&fixture, &mut harness);
    let collated = collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    let highest = ids(harness.pool.query().from_highest_priority());
    assert_eq!(ids(&collated.candidates), highest);
    assert!(collated.removed.is_empty());
    // `exclude` skips a transaction; the sender's later ones still follow it.
    let collated = collate(&mut harness.pool, &ctx, false, &[*txs[0].id()]).unwrap();
    assert_eq!(
        ids(&collated.candidates),
        [id(&txs[2]), id(&txs[1]), id(&txs[3]), id(&txs[4])]
    );
    assert_eq!(harness.pool.size(), 5);
}

#[test]
fn the_block_limits_are_the_tips() {
    let fixture = Fixture::new();
    let mut harness = Harness::new("limits", |_| {});
    let txs = populate(&fixture, &mut harness);
    let size = txs[0].bytes().len() as u32;
    // At the tip, room for two transactions (each with its 4-byte length) and a byte less than
    // a third; from the next height, room for everything.
    let tight = milestones_with(
        "",
        &format!(
            r#",{{"height":{TIP},"block":{{"version":0,"maxTransactions":150,"maxPayload":{}}}}},
            {{"height":{},"block":{{"version":0,"maxTransactions":150,"maxPayload":2097152}}}}"#,
            141 + 3 * (size + 4) - 1,
            TIP + 1
        ),
    );
    let ctx = fixture.ctx(&tight);
    let collated = collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    assert_eq!(ids(&collated.candidates), [id(&txs[0]), id(&txs[2])]);
    let counted = milestones_with(
        "",
        &format!(
            r#",{{"height":{TIP},"block":{{"version":0,"maxTransactions":3,"maxPayload":2097152}}}},
            {{"height":{},"block":{{"version":0,"maxTransactions":150,"maxPayload":2097152}}}}"#,
            TIP + 1
        ),
    );
    let ctx = fixture.ctx(&counted);
    let collated = collate(&mut harness.pool, &ctx, false, &[]).unwrap();
    assert_eq!(
        ids(&collated.candidates),
        [id(&txs[0]), id(&txs[2]), id(&txs[1])]
    );
}

#[test]
fn the_byte_budget_stops_the_collation() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("budget", |_| {});
    let small = fixture.alice.transfer(&fixture.bob, 1, 2, 3 * FEE);
    let items: Vec<_> = (0..40)
        .map(|n| heartwood_crypto::transactions::types::TransferItem {
            amount: n + 1,
            recipient: fixture.carol.address,
        })
        .collect();
    let big = fixture.bob.sign(
        heartwood_crypto::transactions::TransactionBuilder::transfer(items)
            .nonce(1)
            .fee(2 * FEE),
    );
    let last = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    harness.submit(&ctx, &[&small, &big, &last]);
    let room = 141 + (small.bytes().len() + 4) as u32 + (last.bytes().len() + 4) as u32;
    let milestones = milestones_with(
        "",
        &format!(
            r#",{{"height":{TIP},"block":{{"version":0,"maxTransactions":150,"maxPayload":{room}}}}},
            {{"height":{},"block":{{"version":0,"maxTransactions":150,"maxPayload":2097152}}}}"#,
            TIP + 1
        ),
    );
    let ctx = fixture.ctx(&milestones);
    // The big one does not fit, and the collation stops there although the last one would fit.
    let collated = collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    assert_eq!(ids(&collated.candidates), [id(&small)]);
}

#[test]
fn a_failed_sender_is_skipped_and_removed() {
    let mut fixture = Fixture::new();
    let mut harness = Harness::new("failed", |_| {});
    let txs = populate(&fixture, &mut harness);
    // A block elsewhere spent alice's nonce 2: her pool transactions no longer apply.
    fixture
        .wallets
        .get_or_create(&fixture.alice.address)
        .set_nonce(2);
    let ctx = fixture.ctx(&MILESTONES);
    let collated = collate(&mut harness.pool, &ctx, false, &[]).unwrap();
    assert_eq!(
        collated.candidates.len(),
        5,
        "without validate nothing fails"
    );
    let collated = collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    assert_eq!(
        ids(&collated.candidates),
        [id(&txs[2]), id(&txs[3]), id(&txs[4])]
    );
    assert_eq!(shared_ids(&collated.removed), [id(&txs[1]), id(&txs[0])]);
    assert_eq!(harness.all(), [id(&txs[3]), id(&txs[2]), id(&txs[4])]);
    assert_eq!(harness.stored(), [id(&txs[2]), id(&txs[3]), id(&txs[4])]);
}

#[test]
fn validation_shares_one_copy_of_the_wallets() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("shared", |_| {});
    // Three of the five delegates resign, each in its own view, where four would remain. On the
    // one copy the collation validates with, the third would leave two for three places.
    let resignations: Vec<Transaction> = fixture.delegates[2..]
        .iter()
        .zip([3, 2, 1])
        .map(|(delegate, fees)| delegate.sign(resignation(None).nonce(2).fee(fees * FEE)))
        .collect();
    let refs: Vec<&Transaction> = resignations.iter().collect();
    assert_eq!(harness.submit(&ctx, &refs).accept.len(), 3);
    let collated = collate(&mut harness.pool, &ctx, false, &[]).unwrap();
    assert_eq!(collated.candidates.len(), 3);
    let collated = collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    assert_eq!(ids(&collated.candidates), ids(&resignations[..2]));
    assert_eq!(shared_ids(&collated.removed), [id(&resignations[2])]);
    assert_eq!(harness.pool.size(), 2);
}
