//! The dynamic fee floor of a decoded transaction ([`minimum_fee`], a wrapper of
//! `heartwood-crypto`'s fee floor): each type, milestone variants and the height the floor is
//! read at. The golden vectors of the floor (V19, and the `fee.minimum` records of V06) are
//! checked in `heartwood-crypto`, and through this wrapper by difftest's L0.

mod support;

use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{Memo, ResignationType};
use heartwood_crypto::{Milestones, Transaction};
use heartwood_transactions::minimum_fee;
use support::*;

/// The byte length rounded as the reference's `Math.round(size / 2)` rounds it.
fn half(transaction: &Transaction) -> i128 {
    i128::try_from(transaction.bytes().len().div_ceil(2)).unwrap()
}

fn with_fees(table: &str) -> Milestones {
    milestones_with(&format!(r#","dynamicFees":{table}"#))
}

#[test]
fn the_floor_of_each_type() {
    let sender = Account::new("sender");
    let to = Account::new("recipient");
    let params = params();
    // An odd and an even size: the half is rounded up.
    let odd = sender.sign(transfer(&to, 1).nonce(1).fee(FEE));
    let even = sender.sign(
        transfer(&to, 1)
            .nonce(1)
            .fee(FEE)
            .memo(Memo::new("x").unwrap()),
    );
    assert_ne!(odd.bytes().len() % 2, even.bytes().len() % 2);
    for tx in [&odd, &even] {
        assert_eq!(minimum_fee(tx, params), (10 + half(tx)) * 100);
    }
    let vote = sender.sign(
        TransactionBuilder::vote(votes(&[("del_a", 10_000)]))
            .nonce(1)
            .fee(FEE),
    );
    assert_eq!(minimum_fee(&vote, params), (20 + half(&vote)) * 100);
    let register = sender.sign(
        TransactionBuilder::delegate_registration("del_a")
            .nonce(1)
            .fee(FEE),
    );
    assert_eq!(
        minimum_fee(&register, params),
        (1000 + half(&register)) * 100
    );
    // No addon for the type: the size alone.
    let second = Account::new("second");
    let second_signature = sender.sign(
        TransactionBuilder::second_signature(second.public_key().to_compressed())
            .nonce(1)
            .fee(FEE),
    );
    assert_eq!(
        minimum_fee(&second_signature, params),
        half(&second_signature) * 100
    );
    // Burns and resignations have no floor.
    let burn = sender.sign(TransactionBuilder::burn(BURN_MINIMUM).nonce(1).fee(FEE));
    assert_eq!(minimum_fee(&burn, params), 0);
    for resignation_type in [
        None,
        Some(ResignationType::PermanentResign),
        Some(ResignationType::NotResigned),
    ] {
        let resign = sender.sign(resignation(resignation_type).nonce(1).fee(FEE));
        assert_eq!(minimum_fee(&resign, params), 0);
    }
}

#[test]
fn the_floor_under_milestone_variants() {
    let sender = Account::new("sender");
    let tx = sender.sign(transfer(&Account::new("recipient"), 1).nonce(1).fee(FEE));
    let floor = |table: &str| minimum_fee(&tx, with_fees(table).at(1));
    assert_eq!(floor("null"), 0);
    assert_eq!(
        floor(r#"{"enabled":false,"minFee":100,"addonBytes":{"transfer":10}}"#),
        0
    );
    assert_eq!(floor(r#"{"minFee":100,"addonBytes":{"transfer":10}}"#), 0);
    // A byte price below 1 is 1.
    for min_fee in ["0", "-5", "1"] {
        assert_eq!(
            floor(&format!(
                r#"{{"enabled":true,"minFee":{min_fee},"addonBytes":{{"transfer":10}}}}"#
            )),
            10 + half(&tx)
        );
    }
    assert_eq!(
        floor(r#"{"enabled":true,"minFee":7,"addonBytes":{"vote":10}}"#),
        half(&tx) * 7
    );
    assert_eq!(
        floor(r#"{"enabled":true,"minFee":7,"addonBytes":{"transfer":-1000}}"#),
        (half(&tx) - 1000) * 7
    );
    // The sum is a JavaScript number: past 2^53 it rounds to an even neighbour, the product is
    // exact.
    let big = 9_007_199_254_740_991_i128;
    let sum = big + half(&tx);
    let rounded = if sum % 2 == 0 {
        sum
    } else if (sum - 1) % 4 == 0 {
        sum - 1
    } else {
        sum + 1
    };
    assert_eq!(
        floor(
            r#"{"enabled":true,"minFee":9007199254740991,"addonBytes":{"transfer":9007199254740991}}"#
        ),
        rounded * big
    );
}

#[test]
fn the_floor_is_read_at_the_height() {
    let milestones = milestones_with(r#"},{"height":10,"dynamicFees":{"minFee":1000}"#);
    let sender = Account::new("sender");
    let tx = sender.sign(transfer(&Account::new("recipient"), 1).nonce(1).fee(FEE));
    assert_eq!(minimum_fee(&tx, milestones.at(9)), (10 + half(&tx)) * 100);
    assert_eq!(minimum_fee(&tx, milestones.at(10)), (10 + half(&tx)) * 1000);
}
