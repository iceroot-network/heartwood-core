//! The block and missed-block listings over the committed chain's store: `GET /blocks`,
//! `GET /blocks/missed`, `GET /delegates/{id}/blocks` and `GET /delegates/{id}/blocks/missed`.
//! They run with the whole crate (`integration`).

mod history_fixture;

use std::fs;
use std::path::PathBuf;

use heartwood_api::controllers::{blocks, delegate_history};
use heartwood_api::{ApiError, ApiReply, RouteId};
use heartwood_crypto::Block;
use heartwood_crypto::transactions::types::Asset;
use serde_json::{Value, json};

use history_fixture::{Fixture, fixture, request};

/// A validated query of a block route with `criteria` over the defaults.
fn query(criteria: Value) -> Value {
    let mut query = json!({
        "transform": true,
        "page": 1,
        "limit": 100,
        "orderBy": ["height:desc"],
    });
    for (key, value) in criteria.as_object().unwrap() {
        query[key] = value.clone();
    }
    query
}

fn missed_query(criteria: Value) -> Value {
    let mut query = query(json!({"orderBy": ["timestamp:desc"]}));
    query.as_object_mut().unwrap().remove("transform");
    for (key, value) in criteria.as_object().unwrap() {
        query[key] = value.clone();
    }
    query
}

async fn list(route: RouteId, params: &[(&str, &str)], query: Value) -> Result<ApiReply, ApiError> {
    let ctx = fixture().ctx.clone();
    let request = request(route, params, query);
    match route {
        RouteId::BlocksIndex => blocks::index(ctx, request).await,
        RouteId::BlocksMissed => blocks::missed(ctx, request).await,
        RouteId::DelegatesBlocks => delegate_history::blocks(ctx, request).await,
        _ => delegate_history::missed_blocks(ctx, request).await,
    }
}

async fn blocks_of(query: Value) -> ApiReply {
    list(RouteId::BlocksIndex, &[], query).await.unwrap()
}

fn heights(reply: &ApiReply) -> Vec<u64> {
    reply.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["height"].as_u64().unwrap())
        .collect()
}

fn total(reply: &ApiReply) -> u64 {
    reply.body["meta"]["totalCount"].as_u64().unwrap()
}

fn estimate(reply: &ApiReply) -> bool {
    reply.body["meta"]["totalCountIsEstimate"]
        .as_bool()
        .unwrap()
}

/// The generator's name of the block at `height`, from the reference's stored columns.
fn username(fixture: &Fixture, height: u32) -> Option<&str> {
    match height {
        1 => None,
        _ => fixture.meta[height as usize - 2]["username"].as_str(),
    }
}

/// The transfers' amounts of `block`.
fn transferred(block: &Block) -> u128 {
    block
        .transactions()
        .iter()
        .filter_map(|transaction| match &transaction.data().asset {
            Asset::Transfer { transfers } => Some(transfers),
            _ => None,
        })
        .flatten()
        .map(|item| u128::from(item.amount))
        .sum()
}

/// Every raw block of the chain by height, read through the list.
async fn every_block() -> Vec<Value> {
    let mut all = Vec::new();
    for page in 1..=3 {
        let reply = blocks_of(query(
            json!({"transform": false, "page": page, "orderBy": ["height:asc"]}),
        ))
        .await;
        all.extend(reply.body["data"].as_array().unwrap().iter().cloned());
    }
    all
}

#[tokio::test]
#[ignore = "integration"]
async fn the_list_pages_through_every_block_newest_first() {
    let fixture = fixture();
    let tip = u64::from(fixture.tip());
    let first = blocks_of(query(json!({}))).await;
    assert_eq!(heights(&first), (tip - 99..=tip).rev().collect::<Vec<_>>());
    assert_eq!((total(&first), estimate(&first)), (tip, false));
    let second = blocks_of(query(json!({"page": 2}))).await;
    assert_eq!(
        heights(&second),
        (tip - 199..=tip - 100).rev().collect::<Vec<_>>()
    );
    let offset = blocks_of(query(json!({"offset": 260, "limit": 10}))).await;
    assert_eq!(heights(&offset), [5, 4, 3, 2, 1]);
    // The transformed resource: what the block forged, its generator and its confirmations.
    for data in first.body["data"].as_array().unwrap() {
        let height = u32::try_from(data["height"].as_u64().unwrap()).unwrap();
        let block = fixture.block(height);
        let amount = u128::from(block.header().total_amount) + transferred(block);
        assert_eq!(data["id"], json!(block.id().to_hex()));
        assert_eq!(
            data["forged"]["amount"],
            json!(amount.to_string()),
            "{height}"
        );
        assert_eq!(data["confirmations"], json!(fixture.tip() - height));
        assert_eq!(
            data["generator"]["username"],
            json!(username(fixture, height))
        );
    }
    // Sorted ascending and by two keys.
    let ascending = blocks_of(query(json!({"orderBy": ["height:asc"]}))).await;
    assert_eq!(heights(&ascending), (1..=100).collect::<Vec<_>>());
    let by_name = blocks_of(query(json!({"orderBy": ["username:asc", "height:desc"]}))).await;
    let names: Vec<Option<&str>> = heights(&by_name)
        .iter()
        .map(|h| username(fixture, u32::try_from(*h).unwrap()))
        .collect();
    assert!(names.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[tokio::test]
#[ignore = "integration"]
async fn the_raw_list_is_the_models_with_an_exact_count_flagged_as_an_estimate() {
    let fixture = fixture();
    let reply = blocks_of(query(json!({"transform": false, "limit": 5}))).await;
    // The reference's raw path is asked for an estimate and counts exactly.
    assert_eq!(
        (total(&reply), estimate(&reply)),
        (u64::from(fixture.tip()), true)
    );
    let block = &reply.body["data"][0];
    assert_eq!(block["height"], json!(fixture.tip()));
    assert!(block.get("forged").is_none() && block.get("confirmations").is_none());
    assert!(block["donations"].is_object());
    assert_eq!(block["username"], json!(username(fixture, fixture.tip())));
    let transformed = blocks_of(query(json!({"limit": 5}))).await;
    assert!(!estimate(&transformed));
    assert!(transformed.body["data"][0].get("forged").is_some());
}

#[tokio::test]
#[ignore = "integration"]
async fn every_criterion_selects_the_blocks_that_hold_it() {
    let all = every_block().await;
    assert_eq!(all.len(), 265);
    let sample = &all[99];
    let number = |value: &Value| -> Value {
        match value {
            Value::String(text) => json!(text.parse::<u64>().unwrap()),
            other => other.clone(),
        }
    };
    let keys = [
        "id",
        "version",
        "timestamp",
        "previousBlock",
        "height",
        "numberOfTransactions",
        "totalAmount",
        "totalFee",
        "burnedFee",
        "reward",
        "payloadLength",
        "payloadHash",
        "generatorPublicKey",
        "username",
        "blockSignature",
    ];
    for key in keys {
        let given = if sample[key].is_string()
            && sample[key].as_str().unwrap().len() < 20
            && key != "username"
        {
            number(&sample[key])
        } else {
            sample[key].clone()
        };
        let expected: Vec<u64> = all
            .iter()
            .filter(|block| block[key] == sample[key])
            .map(|block| block["height"].as_u64().unwrap())
            .rev()
            .collect();
        assert!(!expected.is_empty(), "{key}");
        let reply = blocks_of(query(json!({key: given}))).await;
        assert_eq!(
            heights(&reply),
            expected[..expected.len().min(100)],
            "{key}"
        );
        assert_eq!(total(&reply), expected.len() as u64, "{key}");
    }
    // A list of values holds when one of them does; a range holds between its ends.
    let reply = blocks_of(query(json!({"height": [3, 40, 999]}))).await;
    assert_eq!(heights(&reply), [40, 3]);
    let reply = blocks_of(query(json!({"height": {"from": 10, "to": 14}}))).await;
    assert_eq!(heights(&reply), [14, 13, 12, 11, 10]);
    let reply = blocks_of(query(json!({"height": {"from": 260}}))).await;
    assert_eq!(heights(&reply), [265, 264, 263, 262, 261, 260]);
    let both = json!({"height": {"from": 10, "to": 60}, "numberOfTransactions": 0});
    let empty: Vec<u64> = all
        .iter()
        .filter(|b| (10..=60).contains(&b["height"].as_u64().unwrap()))
        .filter(|b| b["numberOfTransactions"] == 0)
        .map(|b| b["height"].as_u64().unwrap())
        .rev()
        .collect();
    assert_eq!(heights(&blocks_of(query(both)).await), empty);
    // An empty list of values holds nothing.
    let reply = blocks_of(query(json!({"id": []}))).await;
    assert_eq!((heights(&reply).len(), total(&reply)), (0, 0));
}

fn reference_missed() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/difftest/corpus/txgen-one-node/reference/missed-blocks.jsonl");
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The `(timestamp, height, username)` of each row of a missed-slot page.
fn missed_rows(reply: &ApiReply) -> Vec<(u64, u64, String)> {
    reply.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["timestamp"]["epoch"].as_u64().unwrap(),
                row["height"].as_u64().unwrap(),
                row["username"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The reference's missed slots that `keep` keeps, newest first.
fn expected_missed(keep: impl Fn(&Value) -> bool) -> Vec<(u64, u64, String)> {
    let mut rows: Vec<(u64, u64, String)> = reference_missed()
        .iter()
        .filter(|row| keep(row))
        .map(|row| {
            (
                row["timestamp"].as_u64().unwrap(),
                row["height"].as_u64().unwrap(),
                row["username"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    rows
}

async fn missed_of(query: Value) -> ApiReply {
    list(RouteId::BlocksMissed, &[], query).await.unwrap()
}

#[tokio::test]
#[ignore = "integration"]
async fn the_missed_slots_are_the_references_newest_first() {
    let expected = expected_missed(|_| true);
    assert_eq!(expected.len(), 8);
    let reply = missed_of(missed_query(json!({}))).await;
    assert_eq!(missed_rows(&reply), expected);
    assert_eq!((total(&reply), estimate(&reply)), (8, false));
    // The timestamp is the epoch/unix/human triple.
    let row = &reply.body["data"][0];
    assert_eq!(row["timestamp"]["epoch"], json!(1704));
    assert!(row["timestamp"]["unix"].is_number() && row["timestamp"]["human"].is_string());
    assert_eq!(row.as_object().unwrap().len(), 3);
    let ascending = missed_of(missed_query(json!({"orderBy": ["timestamp:asc"]}))).await;
    let mut oldest = expected.clone();
    oldest.reverse();
    assert_eq!(missed_rows(&ascending), oldest);
    let by_height = missed_of(missed_query(json!({"orderBy": ["height:desc"]}))).await;
    assert_eq!(missed_rows(&by_height).first().unwrap().1, 207);
    let paged = missed_of(missed_query(json!({"page": 2, "limit": 3}))).await;
    assert_eq!(missed_rows(&paged), expected[3..6]);
    assert_eq!(total(&paged), 8);
}

#[tokio::test]
#[ignore = "integration"]
async fn each_missed_slot_criterion_selects_the_slots_that_hold_it() {
    let by = |query| async move { missed_rows(&missed_of(missed_query(query)).await) };
    let name = by(json!({"username": "tx1n2290"})).await;
    assert_eq!(name, expected_missed(|r| r["username"] == "tx1n2290"));
    assert_eq!(name.len(), 3);
    let height = by(json!({"height": 181})).await;
    assert_eq!(height, expected_missed(|r| r["height"] == 181));
    assert_eq!(height.len(), 5);
    assert_eq!(
        by(json!({"timestamp": 1456})).await,
        expected_missed(|r| r["timestamp"] == 1456)
    );
    let range = json!({"timestamp": {"from": 1280, "to": 1464}});
    let inside = |r: &Value| (1280..=1464).contains(&r["timestamp"].as_u64().unwrap());
    assert_eq!(by(range).await, expected_missed(inside));
    let several = by(json!({"height": [154, 207]})).await;
    assert_eq!(
        several,
        expected_missed(|r| r["height"] == 154 || r["height"] == 207)
    );
    let both = by(json!({"username": "tx1n2290", "height": {"from": 160}})).await;
    assert_eq!(
        both,
        expected_missed(|r| r["username"] == "tx1n2290" && r["height"] != 154)
    );
    assert!(by(json!({"height": 999})).await.is_empty());
    assert!(by(json!({"username": []})).await.is_empty());
}

fn address_of(fixture: &Fixture, name: &str) -> String {
    let wallet = heartwood_api::controllers::wallets::find(&fixture.ctx.state(), name).unwrap();
    wallet.address().to_base58()
}

#[tokio::test]
#[ignore = "integration"]
async fn a_delegates_blocks_are_the_blocks_it_forged() {
    let fixture = fixture();
    let name = username(fixture, 2).unwrap().to_owned();
    let expected: Vec<u64> = (2..=fixture.tip())
        .filter(|h| username(fixture, *h) == Some(name.as_str()))
        .map(u64::from)
        .rev()
        .collect();
    assert!(!expected.is_empty());
    let key = fixture.block(2).header().generator_public_key.to_hex();
    let address = address_of(fixture, &name);
    for id in [name.clone(), key, address] {
        let reply = list(RouteId::DelegatesBlocks, &[("id", &id)], query(json!({})))
            .await
            .unwrap();
        assert_eq!(heights(&reply), expected, "{id}");
        assert_eq!(
            (total(&reply), estimate(&reply)),
            (expected.len() as u64, false)
        );
        assert!(reply.body["data"][0].get("forged").is_some());
    }
    // A `username` of the query is the delegate's; other criteria narrow.
    let other = username(fixture, 3).unwrap();
    let query_of = query(json!({"username": other}));
    let reply = list(RouteId::DelegatesBlocks, &[("id", &name)], query_of)
        .await
        .unwrap();
    assert_eq!(heights(&reply), expected);
    let from = expected[0];
    let narrow = query(json!({"height": {"to": from - 1}}));
    let reply = list(RouteId::DelegatesBlocks, &[("id", &name)], narrow)
        .await
        .unwrap();
    assert_eq!(heights(&reply), expected[1..]);
    // Raw models: the count is exact and flagged as the configuration says.
    let raw = query(json!({"transform": false}));
    let reply = list(RouteId::DelegatesBlocks, &[("id", &name)], raw)
        .await
        .unwrap();
    assert_eq!(heights(&reply), expected);
    assert!(!estimate(&reply));
    assert!(reply.body["data"][0].get("forged").is_none());
}

#[tokio::test]
#[ignore = "integration"]
async fn what_is_no_delegate_has_no_blocks() {
    let fixture = fixture();
    let not_found =
        json!({"statusCode": 404, "error": "Not Found", "message": "Delegate not found"});
    // A wallet that is no delegate, an unknown name, and an id that is no wallet at all.
    let voter = fixture.transactions().into_iter().find_map(|(_, _, t)| {
        let network = fixture.milestones().at(1);
        let address = t.data().sender_address(network).to_base58();
        let wallet = heartwood_api::controllers::wallets::find(&fixture.ctx.state(), &address);
        wallet
            .ok()
            .filter(|w| w.delegate().is_none())
            .map(|_| address)
    });
    let mut ids = vec![
        "nobody".to_owned(),
        "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjZ".to_owned(),
    ];
    ids.extend(voter);
    for route in [RouteId::DelegatesBlocks, RouteId::DelegatesMissedBlocks] {
        for id in &ids {
            let error = list(route, &[("id", id)], query(json!({})))
                .await
                .unwrap_err();
            assert_eq!(error.body(), not_found, "{id}");
        }
    }
    assert!(ids.len() >= 3);
}

#[tokio::test]
#[ignore = "integration"]
async fn a_delegates_missed_slots_are_its_own() {
    let fixture = fixture();
    let missed = |name: &'static str, query: Value| async move {
        let reply = list(RouteId::DelegatesMissedBlocks, &[("id", name)], query)
            .await
            .unwrap();
        (missed_rows(&reply), total(&reply), estimate(&reply))
    };
    let (rows, count, flag) = missed("tx1n2290", missed_query(json!({}))).await;
    assert_eq!(rows, expected_missed(|r| r["username"] == "tx1n2290"));
    assert_eq!((count, flag), (3, false));
    let (rows, ..) = missed("genesis_1", missed_query(json!({}))).await;
    assert_eq!(rows, expected_missed(|r| r["username"] == "genesis_1"));
    // By the delegate's address and public key too.
    let address = address_of(fixture, "genesis_1");
    let by_address = list(
        RouteId::DelegatesMissedBlocks,
        &[("id", &address)],
        missed_query(json!({})),
    )
    .await
    .unwrap();
    assert_eq!(missed_rows(&by_address), rows);
    // Height and timestamp narrow it; a `username` of the query is the delegate's.
    let (rows, ..) = missed("tx1n2290", missed_query(json!({"height": {"from": 160}}))).await;
    assert_eq!(rows.len(), 2);
    let (rows, ..) = missed("tx1n2290", missed_query(json!({"username": "genesis_1"}))).await;
    assert_eq!(rows.len(), 3);
    let (rows, ..) = missed(
        "tx1n2290",
        missed_query(json!({"orderBy": ["timestamp:asc"]})),
    )
    .await;
    assert_eq!(rows.first().unwrap().0, 1224);
}

#[tokio::test]
#[ignore = "integration"]
async fn a_block_key_the_missed_slots_have_no_column_for_gives_an_empty_page() {
    for query in [
        missed_query(json!({"reward": 0})),
        missed_query(json!({"numberOfTransactions": 1, "height": 154})),
        missed_query(json!({"id": "ab".repeat(32)})),
    ] {
        let reply = list(
            RouteId::DelegatesMissedBlocks,
            &[("id", "tx1n2290")],
            query.clone(),
        )
        .await
        .unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body["data"], json!([]), "{query}");
        assert_eq!(total(&reply), 0);
        let reply = list(RouteId::BlocksMissed, &[], query).await.unwrap();
        assert_eq!(reply.body["data"], json!([]));
        assert_eq!(total(&reply), 0);
    }
}
