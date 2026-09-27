//! The block routes over the committed chain's store: `GET /blocks/last`, `GET /blocks/{id}` and
//! `GET /blocks/{id}/transactions`.
//!
//! The raw block (`transform=false`) and every "Block not found" run here; the transformed block
//! and the block's transactions use the resources and the list envelope, so their tests run with
//! the whole crate (`integration`).

mod history_fixture;

use heartwood_api::RouteId;
use heartwood_api::controllers::blocks;
use heartwood_crypto::Block;
use heartwood_crypto::transactions::types::Asset;
use serde_json::{Value, json};

use history_fixture::{Fixture, fixture, request};

/// The raw block the reference's block model holds for `block`, with the columns the reference
/// stored for it (`meta`, from height 2).
fn expected_raw(fixture: &Fixture, block: &Block) -> Value {
    let header = block.header();
    let mut out = serde_json::Map::new();
    out.insert("id".into(), json!(block.id().to_hex()));
    out.insert("version".into(), json!(header.version));
    out.insert("timestamp".into(), json!(header.timestamp));
    let previous = heartwood_crypto::BlockId::from_array(header.previous_block);
    out.insert("previousBlock".into(), json!(previous.to_hex()));
    out.insert("height".into(), json!(header.height));
    out.insert(
        "numberOfTransactions".into(),
        json!(header.number_of_transactions),
    );
    out.insert("totalAmount".into(), json!(header.total_amount.to_string()));
    out.insert("totalFee".into(), json!(header.total_fee.to_string()));
    let (burned, donations, username) = match header.height {
        1 => (json!("0"), json!({}), None),
        height => {
            let meta = &fixture.meta[height as usize - 2];
            assert_eq!(meta["h"], json!(height));
            (
                meta["burnedFee"].clone(),
                meta["donations"].clone(),
                Some(meta["username"].clone()),
            )
        }
    };
    out.insert("burnedFee".into(), burned);
    out.insert("reward".into(), json!(header.reward.to_string()));
    out.insert("donations".into(), donations);
    out.insert("payloadLength".into(), json!(header.payload_length));
    out.insert(
        "payloadHash".into(),
        json!(heartwood_crypto::utils::hex::encode(&header.payload_hash)),
    );
    out.insert(
        "generatorPublicKey".into(),
        json!(header.generator_public_key.to_hex()),
    );
    if let Some(username) = username {
        out.insert("username".into(), username);
    }
    out.insert(
        "blockSignature".into(),
        json!(header.block_signature.to_hex()),
    );
    Value::Object(out)
}

async fn show(id: &str, query: Value) -> Result<Value, heartwood_api::ApiError> {
    let fixture = fixture();
    let request = request(RouteId::BlocksShow, &[("id", id)], query);
    let reply = blocks::show(fixture.ctx.clone(), request).await?;
    assert_eq!(reply.status, 200);
    Ok(reply.body["data"].clone())
}

fn raw() -> Value {
    json!({"transform": false})
}

#[tokio::test]
async fn every_raw_block_holds_the_columns_the_reference_stored() {
    let fixture = fixture();
    for height in 1..=fixture.tip() {
        let expected = expected_raw(fixture, fixture.block(height));
        let by_height = show(&height.to_string(), raw()).await.unwrap();
        // As text, so the keys' order counts too, the donations' included.
        assert_eq!(
            by_height.to_string(),
            expected.to_string(),
            "height {height}"
        );
    }
    // By its whole id, the same block.
    let block = fixture.block(100);
    let by_id = show(&block.id().to_hex(), raw()).await.unwrap();
    assert_eq!(by_id, expected_raw(fixture, block));
}

#[tokio::test]
async fn the_last_block_is_the_tip() {
    let fixture = fixture();
    let request = request(RouteId::BlocksLast, &[], raw());
    let reply = blocks::last(fixture.ctx.clone(), request).await.unwrap();
    let tip = fixture.block(fixture.tip());
    assert_eq!(reply.body, json!({"data": expected_raw(fixture, tip)}));
}

#[tokio::test]
async fn an_id_is_read_as_a_height_when_javascript_reads_it_as_one() {
    let fixture = fixture();
    let height = |data: Value| data["height"].as_u64().unwrap();
    for (id, expected) in [
        ("7", 7),
        ("007", 7),
        (" 7 ", 7),
        ("+7", 7),
        ("7.0", 7),
        ("1e2", 100),
        ("0x10", 16),
        ("0b11", 3),
    ] {
        assert_eq!(height(show(id, raw()).await.unwrap()), expected, "{id:?}");
    }
    // A whole id of decimal digits is a height too.
    let digits = format!("{:0>64}", "5");
    assert_eq!(height(show(&digits, raw()).await.unwrap()), 5);
    assert_eq!(fixture.tip(), 265);
}

#[tokio::test]
async fn what_names_no_stored_block_is_not_found() {
    let fixture = fixture();
    let block = fixture.block(20).id().to_hex();
    let unknown = "ab".repeat(32);
    let names = [
        "266".to_owned(),
        "1000000".to_owned(),
        "0".to_owned(),
        "-3".to_owned(),
        "2.5".to_owned(),
        "abc".to_owned(),
        "Infinity".to_owned(),
        block.to_uppercase(),
        block[..63].to_owned(),
        block[..10].to_owned(),
        unknown,
    ];
    let not_found = json!({"statusCode": 404, "error": "Not Found", "message": "Block not found"});
    for name in &names {
        let error = show(name, raw()).await.unwrap_err();
        assert_eq!(error.body(), not_found, "{name:?}");
        let request = request(
            RouteId::BlocksTransactions,
            &[("id", name)],
            json!({"transform": true}),
        );
        let error = blocks::transactions(fixture.ctx.clone(), request)
            .await
            .unwrap_err();
        assert_eq!(error.body(), not_found, "{name:?}");
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

#[tokio::test]
#[ignore = "integration"]
async fn a_block_shows_what_it_forged_with_its_transfers() {
    let fixture = fixture();
    let transform = json!({"transform": true});
    let mut with_transfers = 0;
    for height in 1..=fixture.tip() {
        let block = fixture.block(height);
        let data = show(&height.to_string(), transform.clone()).await.unwrap();
        let header = block.header();
        let amount = u128::from(header.total_amount) + transferred(block);
        assert_eq!(data["id"], json!(block.id().to_hex()), "height {height}");
        assert_eq!(data["height"], json!(height));
        assert_eq!(
            data["forged"]["amount"],
            json!(amount.to_string()),
            "height {height}"
        );
        assert_eq!(data["confirmations"], json!(fixture.tip() - height));
        assert_eq!(data["transactions"], json!(header.number_of_transactions));
        if height > 1 {
            assert_eq!(
                data["generator"]["username"],
                fixture.meta[height as usize - 2]["username"]
            );
        }
        with_transfers += usize::from(transferred(block) > 0);
    }
    assert!(with_transfers >= 3, "{with_transfers}");
    let last = request(RouteId::BlocksLast, &[], transform);
    let reply = blocks::last(fixture.ctx.clone(), last).await.unwrap();
    assert_eq!(reply.body["data"]["height"], json!(fixture.tip()));
}

#[tokio::test]
#[ignore = "integration"]
async fn a_blocks_transactions_are_listed_without_timestamp_or_confirmations() {
    let fixture = fixture();
    let query = json!({
        "transform": true,
        "page": 1,
        "limit": 100,
        "orderBy": ["timestamp:desc", "sequence:desc"],
    });
    for height in [1, 2, 50, 177, 265] {
        let block = fixture.block(height);
        for id in [height.to_string(), block.id().to_hex()] {
            let request = request(RouteId::BlocksTransactions, &[("id", &id)], query.clone());
            let reply = blocks::transactions(fixture.ctx.clone(), request)
                .await
                .unwrap();
            let total = block.transactions().len();
            assert_eq!(reply.body["meta"]["totalCount"], json!(total), "{id}");
            let data = reply.body["data"].as_array().unwrap();
            let expected: Vec<Value> = block
                .transactions()
                .iter()
                .rev()
                .take(100)
                .map(|t| json!(t.id().to_hex()))
                .collect();
            let ids: Vec<Value> = data.iter().map(|t| t["id"].clone()).collect();
            assert_eq!(ids, expected, "{id}");
            for transaction in data {
                assert_eq!(transaction["confirmations"], json!(0));
                assert_eq!(transaction["blockHeight"], json!(height));
                assert!(transaction.get("timestamp").is_none());
            }
        }
    }
    // The block's id replaces a `blockId` of the query; other criteria narrow the list.
    let block = fixture.block(1);
    let other = fixture.block(2).id().to_hex();
    let query = json!({
        "transform": false,
        "page": 1,
        "limit": 100,
        "orderBy": ["timestamp:desc", "sequence:desc"],
        "blockId": other,
        "type": 2,
    });
    let request = request(RouteId::BlocksTransactions, &[("id", "1")], query);
    let reply = blocks::transactions(fixture.ctx.clone(), request)
        .await
        .unwrap();
    let registrations = block
        .transactions()
        .iter()
        .filter(|t| matches!(t.data().asset, Asset::DelegateRegistration { .. }))
        .count();
    assert_eq!(reply.body["meta"]["totalCount"], json!(registrations));
    assert_eq!(reply.body["data"][0]["blockHeight"], json!(1));
}
