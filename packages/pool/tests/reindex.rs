//! The parties the pool's handler runs leave for the state to re-index: the sender and the
//! recipients of each transaction the handlers apply or revert on a sender's view or on a
//! collation's copy, refused or not, in order, as the reference's handlers re-index them in the
//! state's wallets too; nothing for a refusal before the handler, a forged removal, or the check
//! of a transaction against the pool as an eviction would leave it.

mod support;

use heartwood_crypto::identities::Address;
use heartwood_pool::collate;
use support::*;

#[test]
fn the_handler_s_applies_and_reverts_leave_their_parties_in_order() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("reindex-runs", |_| {});
    let (alice, bob, carol, dave) = (
        fixture.alice.address,
        fixture.bob.address,
        fixture.carol.address,
        fixture.dave.address,
    );
    let admitted = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let unfunded = fixture.dave.transfer(&fixture.carol, 1, 1, FEE);
    let result = harness.submit(&ctx, &[&admitted, &unfunded]);
    assert_eq!(result.accept, [id(&admitted)]);
    assert_eq!(error_of(&result, &id(&unfunded)).0, "ERR_APPLY");
    // The handler refused the second one: its parties are left all the same.
    assert_eq!(fixture.reindexed.take(), [alice, bob, dave, carol]);
    // A refusal before the handler leaves nothing.
    assert!(harness.add(&ctx, &admitted).is_err());
    assert_eq!(fixture.reindexed.take(), Vec::<Address>::new());
    // A collation that validates applies each candidate to its copy; one that does not, nothing.
    collate(&mut harness.pool, &ctx, false, &[]).unwrap();
    assert!(fixture.reindexed.take().is_empty());
    collate(&mut harness.pool, &ctx, true, &[]).unwrap();
    assert_eq!(fixture.reindexed.take(), [alice, bob]);
    // A re-add applies the stored transactions again.
    harness.pool.readd(&ctx, &[]).unwrap();
    assert_eq!(fixture.reindexed.take(), [alice, bob]);
    // A removal reverts; a forged removal does not.
    let second = fixture.alice.transfer(&fixture.carol, 1, 3, FEE);
    harness.add(&ctx, &second).unwrap();
    fixture.reindexed.take();
    let removed = (harness.pool)
        .remove_transaction(&ctx, &alice, second.id())
        .unwrap();
    assert_eq!(shared_ids(&removed), [id(&second)]);
    assert_eq!(fixture.reindexed.take(), [alice, carol]);
    (harness.pool)
        .remove_forged_transaction(&alice, admitted.id())
        .unwrap();
    assert!(fixture.reindexed.take().is_empty());
}

#[test]
fn the_check_before_an_eviction_leaves_nothing() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("reindex-eviction", |options| {
        options.max_transactions_in_pool = 1;
    });
    let (alice, bob, carol) = (
        fixture.alice.address,
        fixture.bob.address,
        fixture.carol.address,
    );
    let lowest = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    harness.add(&ctx, &lowest).unwrap();
    fixture.reindexed.take();
    // A higher fee that the handler refuses, checked before the eviction: nothing is evicted and
    // nothing is left to re-index.
    let refused = fixture.dave.transfer(&fixture.bob, 1, 1, 2 * FEE);
    assert!(harness.add(&ctx, &refused).is_err());
    assert!(fixture.reindexed.take().is_empty());
    // One it admits: the eviction's revert, then its own apply.
    let admitted = fixture.carol.transfer(&fixture.bob, 1, 4, 2 * FEE);
    harness.add(&ctx, &admitted).unwrap();
    assert_eq!(fixture.reindexed.take(), [alice, bob, carol, bob]);
}
