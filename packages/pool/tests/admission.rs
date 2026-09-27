//! Admission: the processor's two loops, the cooldown, and every step of the second loop in the
//! reference's order, each with its code and exact message.

mod support;

use heartwood_crypto::Transaction;
use heartwood_crypto::transactions::TransactionBuilder;
use heartwood_crypto::transactions::types::ResignationType;
use heartwood_pool::{Entry, PoolCode, Processor};
use heartwood_state::wallets::{Resignation, Wallet};
use heartwood_transactions::minimum_fee;
use serde_json::{Value, json};
use support::*;

/// A transfer from `from` whose fee is `fee`, with the same size whatever the fee.
fn transfer_with_fee(fixture: &Fixture, fee: u64) -> Transaction {
    fixture.alice.transfer(&fixture.bob, 1, 2, fee)
}

#[test]
fn the_two_loops_and_their_entry_ids() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("loops", |_| {});
    let good = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let gap = fixture.alice.transfer(&fixture.bob, 1, 9, FEE);
    let from_carol = fixture.carol.transfer(&fixture.bob, 5, 4, FEE);
    let mut wrong_id = from_carol.to_json(params());
    wrong_id["id"] = json!("ff".repeat(32));
    let mut no_id = fixture
        .bob
        .transfer(&fixture.carol, 5, 1, FEE)
        .to_json(params());
    no_id.as_object_mut().unwrap().remove("id");
    let result = harness.process(
        &ctx,
        vec![
            bytes(&gap),
            Entry::Bytes(vec![0xff, 3]),
            bytes(&good),
            Entry::Data(wrong_id),
            Entry::Data(no_id),
            bytes(&good),
        ],
    );
    // First-loop refusals (entry indexes) first, then the second loop's (transaction ids).
    assert_eq!(result.invalid, ["1", "4", "5", &id(&gap)]);
    assert_eq!(result.accept, [id(&good), id(&from_carol)]);
    assert_eq!(result.broadcast, result.accept);
    assert!(result.excess.is_empty());
    let errors = result.errors.as_ref().unwrap();
    assert_eq!(
        errors.keys().collect::<Vec<_>>(),
        ["1", "4", "5", &id(&gap)]
    );
    let (code, message) = error_of(&result, "1");
    assert_eq!(code, "ERR_BAD_DATA");
    assert!(
        message.starts_with("Invalid transaction data: "),
        "{message}"
    );
    assert_eq!(error_of(&result, "4").0, "ERR_BAD_DATA");
    assert_eq!(
        error_of(&result, "5"),
        (
            "ERR_COOLDOWN".into(),
            format!(
                "{} must wait another 30 seconds before trying again",
                text(&good)
            )
        )
    );
    assert_eq!(
        error_of(&result, &id(&gap)),
        (
            "ERR_APPLY".into(),
            format!(
                "{} cannot be applied: Cannot apply a transaction with nonce 9: the sender has nonce 1",
                text(&gap)
            )
        )
    );
    assert_eq!(harness.all(), [id(&good), id(&from_carol)]);
    assert_eq!(harness.stored(), [id(&good), id(&from_carol)]);
    let json = result.to_json();
    assert_eq!(json["accept"], json!([id(&good), id(&from_carol)]));
    assert_eq!(json["errors"]["5"]["type"], "ERR_COOLDOWN");
}

#[test]
fn nothing_refused_has_no_errors() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("no-errors", |_| {});
    let result = harness.submit(&ctx, &[&fixture.alice.transfer(&fixture.bob, 1, 2, FEE)]);
    assert_eq!(result.errors, None);
    assert_eq!(result.to_json().get("errors"), None);
    let empty = harness.process(&ctx, Vec::new());
    assert_eq!(
        empty.to_json(),
        json!({"accept": [], "broadcast": [], "invalid": [], "excess": []})
    );
}

#[test]
fn json_ids_and_the_javascript_order_of_errors() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("json-ids", |_| {});
    let hex = "ab".repeat(32);
    let entries = vec![
        Entry::Data(json!({"id": hex, "fee": "not a number"})),
        Entry::Bytes(Vec::new()),
        Entry::Data(json!({"id": 7, "fee": "x"})),
        Entry::Data(json!({"id": null, "fee": "x"})),
        Entry::Data(json!({"id": [1, null, "a"], "fee": "x"})),
        Entry::Data(json!({"id": {"a": 1}, "fee": "x"})),
        Entry::Data(json!("not an object")),
    ];
    let result = harness.process(&ctx, entries);
    assert_eq!(
        result.invalid,
        [&hex, "1", "7", "3", "1,,a", "[object Object]", "6"]
    );
    let json = result.to_json();
    let keys: Vec<&String> = json["errors"].as_object().unwrap().keys().collect();
    // Array indexes first, in numeric order, then the other keys as first refused.
    assert_eq!(keys, ["1", "3", "6", "7", &hex, "1,,a", "[object Object]"]);
}

#[test]
fn the_cooldown_belongs_to_the_processor() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("cooldown", |_| {});
    let good = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let gap = fixture.alice.transfer(&fixture.bob, 1, 7, FEE);
    assert_eq!(harness.submit(&ctx, &[&good, &gap]).accept, [id(&good)]);
    let wait = |harness: &mut Harness, tx: &Transaction| {
        let result = harness.submit(&ctx, &[tx]);
        error_of(&result, "0")
    };
    let cooling = |seconds: &str, tx: &Transaction| {
        (
            "ERR_COOLDOWN".to_owned(),
            format!(
                "{} must wait another {seconds} before trying again",
                text(tx)
            ),
        )
    };
    assert_eq!(wait(&mut harness, &good), cooling("30 seconds", &good));
    // A transaction refused in the second loop cools down too.
    assert_eq!(wait(&mut harness, &gap), cooling("30 seconds", &gap));
    harness.clock.set(NOW_MS + 1);
    // ⌈ms / 1000⌉: one millisecond later is one second later.
    assert_eq!(wait(&mut harness, &good), cooling("29 seconds", &good));
    harness.clock.set(NOW_MS + 29_000);
    assert_eq!(wait(&mut harness, &good), cooling("1 second", &good));
    harness.clock.set(NOW_MS + 29_001);
    let result = harness.submit(&ctx, &[&good]);
    assert_eq!(
        error_of(&result, &id(&good)),
        (
            "ERR_DUPLICATE".into(),
            format!("{} is already in the pool", text(&good))
        )
    );
    // Another processor (the API's and P2P's each have one) has its own cooldown.
    let mut other = Processor::new();
    let result = other
        .process(&mut harness.pool, &ctx, &harness.clock, vec![bytes(&gap)])
        .unwrap();
    assert_eq!(error_of(&result, &id(&gap)).0, "ERR_APPLY");
}

#[test]
fn fee_floor_of_the_milestone() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("floor", |_| {});
    let floor = u64::try_from(minimum_fee(&transfer_with_fee(&fixture, FEE), params())).unwrap();
    let below = transfer_with_fee(&fixture, floor - 1);
    let at = transfer_with_fee(&fixture, floor);
    let result = harness.submit(&ctx, &[&below, &at]);
    assert_eq!(
        error_of(&result, &id(&below)),
        (
            "ERR_LOW_FEE".into(),
            format!("{} fee is too low to enter the pool", text(&below))
        )
    );
    assert_eq!(result.accept, [id(&at)]);
    assert_eq!(result.broadcast, [id(&at)]);
}

#[test]
fn fee_floor_of_the_pool_and_broadcast() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&POOL_FEES);
    let mut harness = Harness::new("pool-floor", |options| {
        options.dynamic_fees.min_fee_pool = 1_000;
        options.dynamic_fees.min_fee_broadcast = 2_000;
    });
    let size = transfer_with_fee(&fixture, FEE).bytes().len() as u64;
    // (addonBytes.transfer + ⌈size / 2⌉) × minFeePool, with the pool's table.
    let floor = (500 + size.div_ceil(2)) * 1_000;
    let below = transfer_with_fee(&fixture, floor - 1);
    let at = transfer_with_fee(&fixture, floor);
    let result = harness.submit(&ctx, &[&below, &at]);
    assert_eq!(error_of(&result, &id(&below)).0, "ERR_LOW_FEE");
    assert_eq!(result.accept, [id(&at)]);
    assert!(result.broadcast.is_empty(), "below the broadcast floor");
    let rich = fixture.carol.transfer(&fixture.bob, 1, 4, floor * 2);
    let result = harness.submit(&ctx, &[&rich]);
    assert_eq!((result.accept.len(), result.broadcast.len()), (1, 1));
}

#[test]
fn static_fees_must_be_paid_exactly() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&POOL_FEES);
    let mut harness = Harness::new("static", |options| options.dynamic_fees.enabled = false);
    let low = transfer_with_fee(&fixture, 9_999_999);
    let high = transfer_with_fee(&fixture, 10_000_001);
    let exact = transfer_with_fee(&fixture, 10_000_000);
    let result = harness.submit(&ctx, &[&low, &high, &exact]);
    assert_eq!(error_of(&result, &id(&low)).0, "ERR_LOW_FEE");
    assert_eq!(
        error_of(&result, &id(&high)),
        (
            "ERR_HIGH_FEE".into(),
            format!("{} fee is too high to enter the pool", text(&high))
        )
    );
    assert_eq!(result.accept, [id(&exact)]);
    // A type without a static fee in the milestone pays its default: 25 for a registration.
    let registration = fixture.bob.sign(
        TransactionBuilder::delegate_registration("bobby")
            .nonce(1)
            .fee(2_500_000_000),
    );
    assert_eq!(
        harness.submit(&ctx, &[&registration]).accept,
        [id(&registration)]
    );
}

#[test]
fn a_full_pool_refuses_a_fee_not_above_the_lowest() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("full", |options| options.max_transactions_in_pool = 2);
    let high = fixture.alice.transfer(&fixture.bob, 1, 2, 3 * FEE);
    let low = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    assert_eq!(harness.submit(&ctx, &[&high, &low]).accept.len(), 2);
    let equal = fixture.bob.transfer(&fixture.carol, 1, 1, FEE);
    let result = harness.submit(&ctx, &[&equal]);
    assert_eq!(
        error_of(&result, &id(&equal)),
        (
            "ERR_POOL_FULL".into(),
            format!(
                "{} fee 0.1 dRT is lower than 0.1 dRT which is already in the pool",
                text(&equal)
            )
        )
    );
    // A higher fee evicts the lowest, with its storage row.
    let higher = fixture.bob.transfer(&fixture.carol, 1, 1, FEE + 1);
    assert_eq!(harness.submit(&ctx, &[&higher]).accept, [id(&higher)]);
    assert_eq!(harness.all(), [id(&high), id(&higher)]);
    assert_eq!(harness.stored(), [id(&high), id(&higher)]);
}

#[test]
fn eviction_takes_the_earliest_sender_on_a_tie() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("tie", |options| options.max_transactions_in_pool = 3);
    let a = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let c = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    let d = fixture.delegates[1].transfer(&fixture.bob, 1, 2, 2 * FEE);
    assert_eq!(harness.submit(&ctx, &[&a, &c, &d]).accept.len(), 3);
    let new = fixture.bob.transfer(&fixture.carol, 1, 1, 2 * FEE);
    assert_eq!(harness.submit(&ctx, &[&new]).accept, [id(&new)]);
    assert_eq!(harness.all(), [id(&c), id(&d), id(&new)]);
}

#[test]
fn a_refused_transaction_evicts_nothing() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("no-evict", |options| options.max_transactions_in_pool = 2);
    let a = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let c = fixture.carol.transfer(&fixture.bob, 1, 4, 2 * FEE);
    harness.submit(&ctx, &[&a, &c]);
    // A higher fee but a nonce gap: refused before anything is evicted.
    let gap = fixture.bob.transfer(&fixture.carol, 1, 5, 5 * FEE);
    let result = harness.submit(&ctx, &[&gap]);
    assert_eq!(
        error_of(&result, &id(&gap)).1,
        format!(
            "{} cannot be applied: Cannot apply a transaction with nonce 5: the sender has nonce 0",
            text(&gap)
        )
    );
    assert_eq!(harness.all(), [id(&a), id(&c)]);
    assert_eq!(harness.stored(), [id(&a), id(&c)]);
    // The per-sender limit is checked before eviction too.
    let mut harness = Harness::new("no-evict-count", |options| {
        options.max_transactions_in_pool = 2;
        options.max_transactions_per_sender = 1;
    });
    harness.submit(&ctx, &[&a, &c]);
    let second = fixture.carol.transfer(&fixture.bob, 1, 5, 5 * FEE);
    let result = harness.submit(&ctx, &[&second]);
    assert_eq!(error_of(&result, &id(&second)).0, "ERR_EXCEEDS_MAX_COUNT");
    assert_eq!(result.excess, [id(&second)]);
    assert_eq!(harness.all(), [id(&a), id(&c)]);
}

#[test]
fn a_sender_may_replace_its_own_lowest_transaction() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("replace", |options| options.max_transactions_in_pool = 2);
    let first = fixture.alice.transfer(&fixture.bob, 1, 2, 3 * FEE);
    let second = fixture.alice.transfer(&fixture.bob, 1, 3, 2 * FEE);
    harness.submit(&ctx, &[&first, &second]);
    // The sender's newest is the lowest: a higher fee for its nonce replaces it, as it does
    // once the reference has evicted it.
    let replacement = fixture.alice.transfer(&fixture.carol, 2, 3, 4 * FEE);
    assert_eq!(
        harness.submit(&ctx, &[&replacement]).accept,
        [id(&replacement)]
    );
    assert_eq!(harness.all(), [id(&replacement), id(&first)]);
    // A nonce that the eviction would not free is refused with the refusal the reference gives
    // after its eviction, and nothing is evicted.
    let gap = fixture.alice.transfer(&fixture.carol, 2, 5, 9 * FEE);
    let result = harness.submit(&ctx, &[&gap]);
    assert_eq!(
        error_of(&result, &id(&gap)).1,
        format!(
            "{} cannot be applied: Cannot apply a transaction with nonce 5: the sender has nonce 2",
            text(&gap)
        )
    );
    assert_eq!(harness.all(), [id(&replacement), id(&first)]);
}

#[test]
fn the_check_before_eviction_sees_the_pool_without_the_evicted() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("evicted-name", |options| {
        options.max_transactions_in_pool = 1
    });
    let register = |account: &Account, nonce, fee| {
        account.sign(
            TransactionBuilder::delegate_registration("wanted")
                .nonce(nonce)
                .fee(fee),
        )
    };
    let low = register(&fixture.alice, 2, FEE);
    assert_eq!(harness.submit(&ctx, &[&low]).accept.len(), 1);
    let high = register(&fixture.carol, 4, 2 * FEE);
    assert_eq!(harness.submit(&ctx, &[&high]).accept, [id(&high)]);
    assert_eq!(harness.all(), [id(&high)]);
}

#[test]
fn the_per_sender_limit_and_allowed_senders() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let txs: Vec<Transaction> = (2..=4)
        .map(|nonce| fixture.alice.transfer(&fixture.bob, 1, nonce, FEE))
        .collect();
    let mut harness = Harness::new("limit", |options| options.max_transactions_per_sender = 2);
    let result = harness.submit(&ctx, &[&txs[0], &txs[1], &txs[2]]);
    assert_eq!(result.excess, [id(&txs[2])]);
    assert_eq!(result.invalid, [id(&txs[2])]);
    assert_eq!(
        error_of(&result, &id(&txs[2])).1,
        format!(
            "{} exceeds the sender's 2 transactions count limit",
            text(&txs[2])
        )
    );
    let mut harness = Harness::new("limit-one", |options| {
        options.max_transactions_per_sender = 1
    });
    let result = harness.submit(&ctx, &[&txs[0], &txs[1]]);
    assert_eq!(
        error_of(&result, &id(&txs[1])).1,
        format!(
            "{} exceeds the sender's 1 transaction count limit",
            text(&txs[1])
        )
    );
    let alice = fixture.alice.address;
    let mut harness = Harness::new("allowed", |options| {
        options.max_transactions_per_sender = 1;
        options.allowed_senders = vec![alice];
    });
    assert_eq!(
        harness
            .submit(&ctx, &[&txs[0], &txs[1], &txs[2]])
            .accept
            .len(),
        3
    );
}

#[test]
fn the_sender_state_steps_in_order() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    // Too large comes before the signature.
    let mut signed = fixture
        .alice
        .transfer(&fixture.bob, 1, 2, FEE)
        .bytes()
        .to_vec();
    let last = signed.len() - 1;
    signed[last] ^= 1;
    let forged = Transaction::decode(&signed, params()).unwrap();
    assert!(!forged.is_verified());
    let size = forged.bytes().len();
    let mut harness = Harness::new("too-large", |options| {
        options.max_transaction_bytes = size - 1
    });
    let result = harness.process(&ctx, vec![Entry::Bytes(signed.clone())]);
    assert_eq!(
        error_of(&result, &id(&forged)),
        (
            "ERR_TOO_LARGE".into(),
            format!(
                "{} exceeds the size limit of {} bytes",
                text(&forged),
                size - 1
            )
        )
    );
    let mut harness = Harness::new("signature", |_| {});
    let result = harness.process(&ctx, vec![Entry::Bytes(signed)]);
    assert_eq!(
        error_of(&result, &id(&forged)),
        (
            "ERR_BAD_DATA".into(),
            format!("{} failed signature verification check", text(&forged))
        )
    );
}

#[test]
fn another_network_byte() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("network", |_| {});
    let mut other = fixture
        .alice
        .transfer(&fixture.bob, 1, 2, FEE)
        .bytes()
        .to_vec();
    other[2] = 30;
    // The strict rules refuse the byte before the pool sees it.
    let result = harness.process(&ctx, vec![Entry::Bytes(other.clone())]);
    assert_eq!(error_of(&result, "0").0, "ERR_BAD_DATA");
    // Given to the second loop, the sender state refuses it.
    let decoded = Transaction::decode_trusted(&other, params()).unwrap();
    assert_eq!(
        harness.add(&ctx, &decoded).unwrap_err().message,
        format!(
            "{} network 30 doesn't match the node's network 90",
            text(&decoded)
        )
    );
}

#[test]
fn handler_refusals_are_err_apply() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("apply", |_| {});
    let broke = fixture.dave.transfer(&fixture.bob, 1, 1, FEE);
    let result = harness.submit(&ctx, &[&broke]);
    assert_eq!(
        error_of(&result, &id(&broke)),
        (
            "ERR_APPLY".into(),
            format!(
                "{} cannot be applied: Insufficient balance in the wallet: tried to spend \
                 0.10000001 dRT but the wallet only has 0 dRT available (-0.10000001 dRT)",
                text(&broke)
            )
        )
    );
    // No refusal leaves a sender mempool or a row behind.
    assert!(harness.pool.mempool().is_empty());
    assert!(harness.stored().is_empty());
}

#[test]
fn pending_and_offline_are_err_apply() {
    let mut fixture = Fixture::new();
    let del_b = &fixture.delegates[1];
    let delegate = fixture
        .wallets
        .wallet_mut(&del_b.address)
        .and_then(Wallet::delegate_mut)
        .unwrap();
    delegate.resigned = Some(Resignation::Temporary);
    delegate.resigned_at = Some(1);
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("pending", |_| {});
    let vote = |account: &Account, nonce, name: &str| {
        account.sign(
            TransactionBuilder::vote(votes(&[(name, 10_000)]))
                .nonce(nonce)
                .fee(FEE),
        )
    };
    let registration = |account: &Account, nonce, name: &str| {
        account.sign(
            TransactionBuilder::delegate_registration(name)
                .nonce(nonce)
                .fee(FEE),
        )
    };
    let second_key = Account::new("second key").public_key().to_compressed();
    let second = |account: &Account, nonce| {
        account.sign(
            TransactionBuilder::second_signature(second_key)
                .nonce(nonce)
                .fee(FEE),
        )
    };
    let (alice, carol) = (&fixture.alice, &fixture.carol);
    let del_c = &fixture.delegates[2];
    let cases = [
        (vote(alice, 2, "del_a"), None),
        (
            vote(alice, 3, "del_a"),
            Some(format!(
                "{} already has a vote transaction in the pool",
                alice.address
            )),
        ),
        (
            vote(carol, 4, "del_c"),
            Some("del_c is not operating a node on the network".to_owned()),
        ),
        // A resigned delegate needs no node.
        (vote(carol, 4, "del_b"), None),
        (registration(carol, 5, "newname"), None),
        (
            registration(carol, 6, "other"),
            Some(format!(
                "{} already has a delegate registration transaction in the pool",
                carol.address
            )),
        ),
        (
            registration(&fixture.bob, 1, "newname"),
            Some("Delegate registration for 'newname' already in the pool".to_owned()),
        ),
        (second(&fixture.bob, 1), None),
        (
            second(&fixture.bob, 2),
            Some(format!(
                "{} already has a second signature registration transaction in the pool",
                fixture.bob.address
            )),
        ),
        (del_c.sign(resignation(None).nonce(2).fee(FEE)), None),
        (
            del_c.sign(
                resignation(Some(ResignationType::PermanentResign))
                    .nonce(3)
                    .fee(FEE),
            ),
            Some("Delegate resignation for 'del_c' already in the pool".to_owned()),
        ),
        (
            del_b.sign(
                resignation(Some(ResignationType::NotResigned))
                    .nonce(2)
                    .fee(FEE),
            ),
            Some("del_b is not operating a node on the network".to_owned()),
        ),
    ];
    for (tx, refusal) in cases {
        let result = harness.submit(&ctx, &[&tx]);
        match refusal {
            None => assert_eq!(result.accept, [id(&tx)], "{result:?}"),
            Some(message) => assert_eq!(
                error_of(&result, &id(&tx)),
                (
                    "ERR_APPLY".into(),
                    format!("{} cannot be applied: {message}", text(&tx))
                )
            ),
        }
    }
}

/// `bytes` of a transfer with its type changed to `kind`, a type this implementation does not
/// carry.
fn removed_type(tx: &Transaction, kind: u8) -> Vec<u8> {
    let mut bytes = tx.bytes().to_vec();
    bytes[7] = kind;
    bytes
}

#[test]
fn removed_types_are_refused_in_the_second_loop() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("removed", |_| {});
    let tx = fixture.alice.transfer(&fixture.bob, 1, 2, FEE);
    let legacy = removed_type(&tx, 0);
    let legacy_id =
        heartwood_crypto::utils::hex::encode(&heartwood_crypto::crypto::hash::sha256(&legacy));
    let mut json = tx.to_json(params());
    json["type"] = json!(4);
    let mut null_id = json.clone();
    null_id["id"] = Value::Null;
    let result = harness.process(
        &ctx,
        vec![
            Entry::Bytes(legacy.clone()),
            Entry::Data(json),
            Entry::Bytes(Vec::new()),
            Entry::Bytes(legacy),
            Entry::Data(null_id),
        ],
    );
    assert_eq!(result.invalid, ["2", "3", &legacy_id, &id(&tx), "4"]);
    let tail = &legacy_id[56..];
    assert_eq!(
        error_of(&result, "3"),
        (
            "ERR_COOLDOWN".into(),
            format!(
                "{}#2 {tail} LegacyTransfer v3 must wait another 30 seconds before trying again",
                fixture.alice.address
            )
        )
    );
    for (entry, kind) in [(legacy_id.as_str(), 0), (&id(&tx), 4), ("4", 4)] {
        assert_eq!(
            error_of(&result, entry),
            (
                "ERR_OTHER".into(),
                format!("Transaction type Core/{kind} is deactivated")
            )
        );
    }
    // Another version byte is refused as the decoder refuses it.
    let mut version = removed_type(&tx, 5);
    version[1] = 2;
    let result = harness.process(&ctx, vec![Entry::Bytes(version)]);
    assert_eq!(error_of(&result, "0").0, "ERR_BAD_DATA");
}

#[test]
fn pool_codes_are_the_reference_strings() {
    assert_eq!(PoolCode::ExceedsMaxCount.as_str(), "ERR_EXCEEDS_MAX_COUNT");
    assert_eq!(PoolCode::Other.to_string(), "ERR_OTHER");
}

#[test]
fn a_later_entry_may_still_evict_what_a_refused_one_did_not() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx(&MILESTONES);
    let mut harness = Harness::new("later-evict", |options| {
        options.max_transactions_in_pool = 1
    });
    let low = fixture.carol.transfer(&fixture.bob, 1, 4, FEE);
    let gap = fixture.alice.transfer(&fixture.bob, 1, 7, 5 * FEE);
    let replacement = fixture.carol.transfer(&fixture.bob, 2, 4, 3 * FEE);
    let result = harness.submit(&ctx, &[&low, &gap, &replacement]);
    // The reference evicts `low` for `gap` and then refuses `gap`; here `gap` is refused first.
    // Either way `replacement` then takes the one place, so the results and the pool agree.
    assert_eq!(result.accept, [id(&low), id(&replacement)]);
    assert_eq!(result.invalid, [id(&gap)]);
    assert_eq!(
        error_of(&result, &id(&gap)).1,
        format!(
            "{} cannot be applied: Cannot apply a transaction with nonce 7: the sender has nonce 1",
            text(&gap)
        )
    );
    assert_eq!(harness.all(), [id(&replacement)]);
    assert_eq!(harness.stored(), [id(&replacement)]);
}
