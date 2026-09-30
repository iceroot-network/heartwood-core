//! `GET /node/fees`: the window of `days` against an injected clock, the query's schema, and the
//! statistics as the reference lists them. The window and the listing are functions of their
//! inputs, so a fixed time stands in for the clock; the route over a store is checked with the
//! whole crate (`integration`).

mod snapshot;

use heartwood_api::RouteId;
use heartwood_api::controllers::node::genesis_as_held;
use heartwood_api::resources::fee_statistics::{in_registry_order, response, window};
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
    // The store gives one row per type in the wire order, zeros for a type with none.
    let rows = vec![
        row(1, 1, 0),
        row(1, 2, 0),
        row(1, 6, 8),
        row(1, 7, 0),
        row(2, 0, 0),
        row(2, 2, 0),
    ];
    let out = response(None, &in_registry_order(rows));
    assert_eq!(out["meta"], json!({}));
    assert_eq!(
        keys(&out["data"]["1"]),
        [
            "delegateRegistration",
            "delegateResignation",
            "transfer",
            "secondSignature"
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
            "delegateRegistration",
            "delegateResignation",
            "transfer",
            "secondSignature"
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
