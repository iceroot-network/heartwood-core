//! `GET /node/fees`: the window of `days` against an injected clock, the query's schema, and the
//! statistics as the reference lists them. The window and the listing are functions of their
//! inputs, so a fixed time stands in for the clock; the route over a store, with the clock given
//! to the handler, is checked with the whole crate (`integration`).

mod history_fixture;
mod snapshot;

use std::collections::BTreeMap;

use heartwood_api::RouteId;
use heartwood_api::controllers::node::{fees_at, genesis_as_held};
use heartwood_api::resources::fee_statistics::{REGISTERED, in_registry_order, response, window};
use heartwood_api::schemas::validate_query;
use heartwood_crypto::crypto::slots;
use heartwood_database::FeeStatisticsRow;
use serde_json::{Value, json};

const DAY_MS: i64 = 86_400_000;

/// The network's epoch as Unix milliseconds, and a clock 40 days and 999 milliseconds after it.
fn clock() -> (i64, i64) {
    let epoch = snapshot::chain().milestones().at(1).epoch();
    let epoch_ms = slots::epoch_unix_ms(epoch).unwrap();
    (epoch_ms, epoch_ms + 40 * DAY_MS + 999)
}

fn row(type_group: u32, kind: u16, fee: u64) -> FeeStatisticsRow {
    FeeStatisticsRow {
        type_group,
        kind,
        avg: fee,
        min: fee,
        max: fee,
        sum: u128::from(fee) * 3,
        burned: u128::from(fee),
    }
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn a_day_counts_back_from_the_clock_and_has_no_end() {
    let (epoch, now) = clock();
    assert_eq!(window(1, now, epoch), Some((39 * 86_400, i64::MAX)));
    assert_eq!(window(30, now, epoch), Some((10 * 86_400, i64::MAX)));
    // A clock a second later moves the start by a second.
    assert_eq!(
        window(1, now + 1_000, epoch),
        Some((39 * 86_400 + 1, i64::MAX))
    );
    // A window that starts before the epoch has negative seconds, as the reference's has.
    assert_eq!(
        window(30, epoch + 5 * DAY_MS, epoch),
        Some((-25 * 86_400, i64::MAX))
    );
}

#[test]
fn the_query_takes_days_from_one_to_thirty_or_nothing() {
    let days = |value: &str| validate_query(RouteId::NodeFees, &json!({ "days": value }));
    assert_eq!(days("1").unwrap(), json!({ "days": 1 }));
    assert_eq!(days("30").unwrap(), json!({ "days": 30 }));
    assert_eq!(
        validate_query(RouteId::NodeFees, &json!({})).unwrap(),
        json!({})
    );
    for refused in ["0", "31", "1.5", "x", ""] {
        assert!(days(refused).is_err(), "{refused}");
    }
}

#[test]
fn meta_holds_the_days_only_when_the_query_gave_them() {
    let rows = [row(1, 6, 10)];
    assert_eq!(response(Some(1), &rows)["meta"], json!({ "days": 1 }));
    assert_eq!(response(Some(30), &rows)["meta"], json!({ "days": 30 }));
    assert_eq!(response(None, &rows)["meta"], json!({}));
}

#[test]
fn a_window_lists_the_types_as_the_rows_come() {
    // The store's order for a window: by type group, then by type.
    let rows = [
        row(1, 1, 1),
        row(1, 2, 2),
        row(1, 6, 3),
        row(1, 7, 4),
        row(2, 0, 5),
        row(2, 2, 6),
    ];
    let out = response(Some(30), &rows);
    assert_eq!(keys(&out["data"]), ["1", "2"]);
    assert_eq!(
        keys(&out["data"]["1"]),
        [
            "secondSignature",
            "delegateRegistration",
            "transfer",
            "delegateResignation"
        ]
    );
    assert_eq!(keys(&out["data"]["2"]), ["burn", "vote"]);
    assert_eq!(
        out["data"]["1"]["transfer"],
        json!({ "avg": "3", "burned": "3", "max": "3", "min": "3", "sum": "9" })
    );
}

#[test]
fn without_days_the_types_come_in_the_registry_order_with_zeros() {
    // The store gives one row per type, zeros for a type with none, in an order of its own.
    let rows = vec![
        row(2, 2, 0),
        row(1, 7, 0),
        row(1, 6, 8),
        row(2, 0, 0),
        row(1, 2, 0),
        row(1, 1, 0),
    ];
    let out = response(None, &in_registry_order(rows));
    assert_eq!(out["meta"], json!({}));
    // The order the reference's service provider binds the handlers in.
    assert_eq!(
        keys(&out["data"]["1"]),
        [
            "secondSignature",
            "delegateRegistration",
            "transfer",
            "delegateResignation"
        ]
    );
    assert_eq!(keys(&out["data"]["2"]), ["burn", "vote"]);
    let zero = json!({ "avg": "0", "burned": "0", "max": "0", "min": "0", "sum": "0" });
    assert_eq!(out["data"]["2"]["burn"], zero);
    assert_eq!(out["data"]["1"]["transfer"]["sum"], "24");
}

#[test]
fn a_type_the_node_does_not_carry_is_left_out() {
    let out = response(Some(1), &[row(1, 8, 1), row(1, 4, 1), row(3, 0, 1)]);
    assert_eq!(out["data"], json!({}));
}

#[test]
fn the_genesis_block_is_held_with_its_big_numbers_as_text() {
    let genesis = json!({
        "id": "g",
        "transactions": [
            {
                "type": 6, "typeGroup": 1, "version": 3, "amount": 0, "fee": 0, "nonce": 1,
                "asset": { "transfers": [{ "amount": 3_400_000_000_000_000_u64, "recipientId": "d" }] },
                "extra": true,
            },
            { "type": 2, "amount": "0", "fee": "7", "nonce": "2", "asset": { "delegate": {} } },
        ],
        "height": 1,
    });
    let held = genesis_as_held(&genesis);
    let transactions = held["transactions"].as_array().unwrap();
    let text = |value: &Value| serde_json::to_string(value).unwrap();
    assert_eq!(
        text(&transactions[0]),
        r#"{"type":6,"typeGroup":1,"version":3,"amount":"0","fee":"0","nonce":"1","asset":{"transfers":[{"amount":"3400000000000000","recipientId":"d"}]},"extra":true,"headerType":0}"#
    );
    assert_eq!(
        text(&transactions[1]),
        r#"{"type":2,"amount":"0","fee":"7","nonce":"2","asset":{"delegate":{}},"version":3,"headerType":0,"typeGroup":1}"#
    );
}

#[tokio::test]
#[ignore = "integration"]
async fn the_route_answers_over_a_store_with_and_without_days() {
    let chain = snapshot::chain();
    let (address, server) = snapshot::serve_chain("fees", chain.pool(Vec::new()), Vec::new()).await;
    let none = snapshot::get(address, "/api/node/fees").await;
    assert_eq!(none.status, 200);
    assert_eq!(none.body["meta"], json!({}));
    assert_eq!(
        keys(&none.body["data"]["1"]),
        [
            "secondSignature",
            "delegateRegistration",
            "transfer",
            "delegateResignation"
        ]
    );
    let month = snapshot::get(address, "/api/node/fees?days=30").await;
    assert_eq!(month.status, 200);
    assert_eq!(month.body["meta"], json!({ "days": 30 }));
    // The store is empty, so a window of days holds no transaction.
    assert_eq!(month.body["data"], json!({}));
    let refused = snapshot::get(address, "/api/node/fees?days=31").await;
    assert_eq!(refused.status, 422);
    server.stop().await;
}

/// The answer of `GET /node/fees` with the validated query `query` over the committed chain's
/// store, at the clock `now_ms`.
async fn fees_over_the_chain(query: Value, now_ms: i64) -> Value {
    let fixture = history_fixture::fixture();
    let request = history_fixture::request(RouteId::NodeFees, &[], query);
    let reply = fees_at(&fixture.ctx, &request, now_ms).await.unwrap();
    assert_eq!(reply.status, 200);
    reply.body
}

/// The statistics the reference writes for `fees`: `AVG()::int8` (rounded half away from zero),
/// the least, the most and the sum, as text.
fn expected(fees: &[u64]) -> Value {
    let count = u128::try_from(fees.len()).unwrap();
    let sum: u128 = fees.iter().copied().map(u128::from).sum();
    let avg = if count == 0 {
        0
    } else {
        (2 * sum + count) / (2 * count)
    };
    json!({
        "avg": avg.to_string(),
        "max": fees.iter().max().copied().unwrap_or(0).to_string(),
        "min": fees.iter().min().copied().unwrap_or(0).to_string(),
        "sum": sum.to_string(),
    })
}

/// `data` without the burned sums, which these tests do not recompute.
fn without_burned(data: &Value) -> Value {
    let mut data = data.clone();
    for types in data.as_object_mut().unwrap().values_mut() {
        for statistics in types.as_object_mut().unwrap().values_mut() {
            statistics.as_object_mut().unwrap().shift_remove("burned");
        }
    }
    data
}

/// The committed chain's fees of the transactions whose block is at `start` or later, by type
/// group and type key, as the window's answer lists them.
fn fees_from(start: i64) -> Value {
    let mut fees: BTreeMap<(u32, u16), Vec<u64>> = BTreeMap::new();
    for (block, _, transaction) in history_fixture::fixture().transactions() {
        if i64::from(block.header().timestamp) >= start {
            let kind = transaction.kind();
            fees.entry((kind.type_group(), kind.type_id()))
                .or_default()
                .push(transaction.data().fee);
        }
    }
    let mut data = serde_json::Map::new();
    for ((group, id), fees) in fees {
        let kind = REGISTERED
            .iter()
            .find(|kind| kind.type_group() == group && kind.type_id() == id)
            .unwrap();
        let types = data.entry(group.to_string()).or_insert_with(|| json!({}));
        types
            .as_object_mut()
            .unwrap()
            .insert(kind.key().to_owned(), expected(&fees));
    }
    Value::Object(data)
}

#[tokio::test]
#[ignore = "integration"]
async fn the_handler_counts_the_days_back_from_the_clock_it_is_given() {
    let fixture = history_fixture::fixture();
    let epoch = slots::epoch_unix_ms(fixture.milestones().at(1).epoch()).unwrap();
    let at = |seconds: i64| epoch + seconds * 1_000;
    let tip = i64::from(fixture.block(fixture.tip()).header().timestamp);

    // Thirty days back from the tip's time reach the genesis block.
    let month = fees_over_the_chain(json!({ "days": 30 }), at(tip)).await;
    assert_eq!(month["meta"], json!({ "days": 30 }));
    assert_eq!(without_burned(&month["data"]), fees_from(i64::MIN));
    assert!(!month["data"].as_object().unwrap().is_empty());

    // A day back from a day after the tip's time holds nothing.
    let later = fees_over_the_chain(json!({ "days": 1 }), at(tip + 1) + DAY_MS).await;
    assert_eq!(later, json!({ "meta": { "days": 1 }, "data": {} }));

    // A day back from a day after a block's time starts at that block, to the second.
    let (middle, _, _) = fixture
        .transactions()
        .into_iter()
        .find(|(block, _, _)| block.height() > fixture.tip() / 2)
        .unwrap();
    let start = i64::from(middle.header().timestamp);
    let day = fees_over_the_chain(json!({ "days": 1 }), at(start) + DAY_MS + 999).await;
    assert_eq!(day["meta"], json!({ "days": 1 }));
    assert_eq!(without_burned(&day["data"]), fees_from(start));
    assert_ne!(fees_from(start), fees_from(i64::MIN));
    let after = fees_over_the_chain(json!({ "days": 1 }), at(start + 1) + DAY_MS).await;
    assert_eq!(without_burned(&after["data"]), fees_from(start + 1));
    assert_ne!(fees_from(start + 1), fees_from(start));
}

#[tokio::test]
#[ignore = "integration"]
async fn without_days_each_kept_type_has_its_last_twenty_transactions() {
    let fixture = history_fixture::fixture();
    let mut transactions = fixture.transactions();
    // The newest first: by height, then by the place in the block.
    transactions.reverse();
    // Whatever the clock, the answer is the same.
    let answer = fees_over_the_chain(json!({}), 0).await;
    assert_eq!(answer["meta"], json!({}));
    let mut data = serde_json::Map::new();
    for kind in REGISTERED {
        let fees: Vec<u64> = transactions
            .iter()
            .filter(|(_, _, transaction)| transaction.kind() == kind)
            .take(20)
            .map(|(_, _, transaction)| transaction.data().fee)
            .collect();
        let types = data
            .entry(kind.type_group().to_string())
            .or_insert_with(|| json!({}));
        types
            .as_object_mut()
            .unwrap()
            .insert(kind.key().to_owned(), expected(&fees));
    }
    assert_eq!(
        serde_json::to_string(&without_burned(&answer["data"])).unwrap(),
        serde_json::to_string(&Value::Object(data)).unwrap()
    );
    assert_eq!(
        keys(&answer["data"]["1"]),
        [
            "secondSignature",
            "delegateRegistration",
            "transfer",
            "delegateResignation"
        ]
    );
}
