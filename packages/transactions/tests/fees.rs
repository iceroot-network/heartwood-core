//! The dynamic fee floor: the formula under milestone variants, and the `fee.minimum` records of
//! the golden vectors (V06 of `heartwood-crypto`, V19 of this crate).

mod golden;
mod support;

use golden::{Devnet, VectorFile};
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::{Memo, ResignationType};
use heartwood_crypto::utils::hex;
use heartwood_crypto::{Milestones, Transaction};
use heartwood_transactions::minimum_fee;
use serde_json::{Value, json};
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

/// The oracle's `fee.minimum` output for the record's transaction under its milestones.
fn fee_minimum(devnet: &Devnet, input: &Value, height: u64) -> Result<Value, String> {
    let milestones = devnet.milestones_of(input)?;
    let params = milestones.at(u32::try_from(height).map_err(|error| error.to_string())?);
    let bytes = hex::decode(input["hex"].as_str().ok_or("hex is not text")?)
        .map_err(|error| error.to_string())?;
    let tx = Transaction::decode(&bytes, params).map_err(|error| error.to_string())?;
    Ok(json!({
        "key": tx.key(),
        "size": tx.bytes().len(),
        "minimumFee": minimum_fee(&tx, params).to_string(),
    }))
}

fn check_fee_records(file: &VectorFile, devnet: &Devnet) -> usize {
    let mut compared = 0;
    for record in file
        .records
        .iter()
        .filter(|record| record.op == "fee.minimum")
    {
        let expected = record.expected.as_ref().unwrap_or_else(|class| {
            panic!(
                "{}: the oracle refused with {class}",
                record.label(&file.class)
            )
        });
        let actual = fee_minimum(devnet, &record.input, record.height)
            .unwrap_or_else(|error| panic!("{}: {error}", record.label(&file.class)));
        assert_eq!(&actual, expected, "{}", record.label(&file.class));
        compared += 1;
    }
    compared
}

#[test]
fn v06_fee_floor_records() {
    let devnet = Devnet::load();
    let file = VectorFile::parse("V06-arithmetic", golden::V06);
    devnet.check(&file);
    assert_eq!(check_fee_records(&file, &devnet), 16);
}
