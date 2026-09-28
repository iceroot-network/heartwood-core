//! The confirmed transaction routes over the committed chain's store: `GET /transactions/{id}`,
//! `GET /transactions` and the history part of `GET /wallets/{id}/transactions`.
//!
//! The raw transaction (`transform=false`) and "Transaction not found" run here; the transformed
//! transaction and the lists use the resources and the list envelope, so their tests run with the
//! whole crate (`integration`).

mod history_fixture;

use heartwood_api::RouteId;
use heartwood_api::controllers::transactions;
use heartwood_crypto::transactions::types::Asset;
use heartwood_crypto::utils::burn;
use heartwood_crypto::{Block, Transaction};
use serde_json::{Value, json};

use history_fixture::{Fixture, fixture, request};

async fn show(id: &str, query: Value) -> Result<Value, heartwood_api::ApiError> {
    let fixture = fixture();
    let request = request(RouteId::TransactionsShow, &[("id", id)], query);
    let reply = transactions::show(fixture.ctx.clone(), request).await?;
    assert_eq!(reply.status, 200);
    Ok(reply.body["data"].clone())
}

/// The keys the raw data of `transaction` has, in order.
fn raw_keys(transaction: &Transaction) -> Vec<&'static str> {
    let data = transaction.data();
    let mut keys = vec![
        "headerType",
        "version",
        "network",
        "typeGroup",
        "type",
        "nonce",
        "senderPublicKey",
        "senderId",
        "fee",
    ];
    if data.memo.is_some() {
        keys.push("memo");
    }
    match &data.asset {
        Asset::Burn { .. } => keys.push("amount"),
        Asset::DelegateResignation {
            resignation_type: None,
        } => {}
        _ => keys.push("asset"),
    }
    if data.signature.is_some() {
        keys.push("signature");
    }
    if data.second_signature.is_some() {
        keys.push("secondSignature");
    }
    keys.extend(["id", "burnedFee", "blockId", "blockHeight", "sequence"]);
    keys
}

#[tokio::test]
async fn every_raw_transaction_is_its_data_with_its_place_in_the_chain() {
    let fixture = fixture();
    let genesis: Value = fixture.ctx.node.crypto.genesis["transactions"].clone();
    let mut kinds = std::collections::BTreeSet::new();
    for (block, sequence, transaction) in fixture.transactions() {
        let id = transaction.id().to_hex();
        let data = show(&id, json!({"transform": false})).await.unwrap();
        let keys: Vec<&str> = data
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, raw_keys(transaction), "{id}");
        let params = fixture.milestones().at(block.height());
        let fee = transaction.fee();
        assert_eq!(data["id"], json!(id));
        assert_eq!(data["typeGroup"], json!(transaction.kind().type_group()));
        assert_eq!(data["version"], json!(transaction.data().version));
        assert_eq!(
            data["burnedFee"],
            json!(burn::burned_fee(fee, params).to_string())
        );
        assert_eq!(data["blockId"], json!(block.id().to_hex()));
        assert_eq!(data["blockHeight"], json!(block.height()));
        assert_eq!(data["sequence"], json!(sequence));
        // The fields of the reference's own genesis file are the same.
        if block.height() == 1 {
            let reference = &genesis[sequence as usize];
            for (key, value) in reference.as_object().unwrap() {
                assert_eq!(&data[key], value, "{id} {key}");
            }
        }
        kinds.insert((
            transaction.kind().type_group(),
            transaction.kind().type_id(),
        ));
    }
    // Every kept type is in the chain.
    assert_eq!(kinds.len(), 6, "{kinds:?}");
}

#[tokio::test]
async fn an_id_that_is_not_stored_as_given_is_not_found() {
    let fixture = fixture();
    let (_, _, transaction) = fixture.transactions()[120];
    let id = transaction.id().to_hex();
    let not_found =
        json!({"statusCode": 404, "error": "Not Found", "message": "Transaction not found"});
    for name in [id.to_uppercase(), "ab".repeat(32), id[..63].to_owned()] {
        let error = show(&name, json!({"transform": false})).await.unwrap_err();
        assert_eq!(error.body(), not_found, "{name}");
    }
}

/// A validated query of `GET /transactions` with `criteria` over the defaults.
fn query(criteria: Value) -> Value {
    let mut query = json!({
        "transform": true,
        "page": 1,
        "limit": 100,
        "orderBy": ["timestamp:desc", "sequence:desc"],
    });
    for (key, value) in criteria.as_object().unwrap() {
        query[key] = value.clone();
    }
    query
}

/// The ids and total of `GET /transactions` with `query`.
async fn index(query: Value) -> (Vec<String>, u64) {
    let fixture = fixture();
    let request = request(RouteId::TransactionsIndex, &[], query);
    let reply = transactions::index(fixture.ctx.clone(), request)
        .await
        .unwrap();
    let ids = reply.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    (ids, reply.body["meta"]["totalCount"].as_u64().unwrap())
}

/// The ids of the chain's transactions that `keep` keeps, newest first (the default order).
fn newest_first(fixture: &Fixture, keep: impl Fn(&Block, &Transaction) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = fixture
        .transactions()
        .into_iter()
        .filter(|(block, _, transaction)| keep(block, transaction))
        .map(|(_, _, transaction)| transaction.id().to_hex())
        .collect();
    ids.reverse();
    ids
}

/// The addresses a transaction pays.
fn recipients(transaction: &Transaction) -> Vec<String> {
    match &transaction.data().asset {
        Asset::Transfer { transfers } => transfers
            .iter()
            .map(|item| item.recipient.to_base58())
            .collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
#[ignore = "integration"]
async fn the_list_pages_through_every_transaction_newest_first() {
    let fixture = fixture();
    let all = newest_first(fixture, |_, _| true);
    assert_eq!(all.len(), 149);
    let (first, total) = index(query(json!({}))).await;
    assert_eq!((first.as_slice(), total), (&all[..100], 149));
    let (second, _) = index(query(json!({"page": 2}))).await;
    assert_eq!(second, all[100..]);
    let (offset, _) = index(query(json!({"offset": 140, "limit": 5}))).await;
    assert_eq!(offset, all[140..145]);
    // The transformed resource: the block's timestamp, and the tip minus its height plus one.
    let request = request(RouteId::TransactionsIndex, &[], query(json!({"limit": 3})));
    let reply = transactions::index(fixture.ctx.clone(), request)
        .await
        .unwrap();
    for transaction in reply.body["data"].as_array().unwrap() {
        let height = transaction["blockHeight"].as_u64().unwrap();
        let block = fixture.block(u32::try_from(height).unwrap());
        assert_eq!(
            transaction["confirmations"],
            json!(u64::from(fixture.tip()) - height + 1)
        );
        assert_eq!(
            transaction["timestamp"]["epoch"],
            json!(block.header().timestamp)
        );
    }
}

#[tokio::test]
#[ignore = "integration"]
async fn the_criteria_select_as_the_reference_selects() {
    let fixture = fixture();
    let network = fixture.milestones().at(1);
    let sender = |t: &Transaction| t.data().sender_address(network).to_base58();
    // A recipient of a transfer that also sends, and one that only receives.
    let transfers: Vec<&Transaction> = fixture
        .transactions()
        .into_iter()
        .map(|(_, _, t)| t)
        .filter(|t| !recipients(t).is_empty())
        .collect();
    let senders: Vec<String> = fixture
        .transactions()
        .iter()
        .map(|(_, _, t)| sender(t))
        .collect();
    let paid: Vec<String> = transfers.iter().flat_map(|t| recipients(t)).collect();
    let both = paid.iter().find(|a| senders.contains(a)).unwrap().clone();
    let only_paid = paid.iter().find(|a| !senders.contains(a)).unwrap().clone();
    for address in [&both, &only_paid] {
        let expected = newest_first(fixture, |_, t| {
            sender(t) == *address || recipients(t).contains(address)
        });
        let (ids, total) = index(query(json!({"address": address}))).await;
        assert_eq!(ids, expected[..expected.len().min(100)], "{address}");
        assert_eq!(total, expected.len() as u64);
        let expected = newest_first(fixture, |_, t| recipients(t).contains(address));
        let (ids, _) = index(query(json!({"recipientId": address}))).await;
        assert_eq!(ids, expected, "{address}");
        let expected = newest_first(fixture, |_, t| sender(t) == *address);
        let (ids, _) = index(query(json!({"senderId": address}))).await;
        assert_eq!(ids, expected, "{address}");
        // The wallet route's history is the address criteria.
        let request = request(
            RouteId::WalletsTransactions,
            &[("id", address)],
            query(json!({})),
        );
        let reply =
            transactions::wallet_transactions(fixture.ctx.clone(), request, address.clone())
                .await
                .unwrap();
        let (ids, _) = index(query(json!({"address": address}))).await;
        let wallet: Vec<&str> = reply.body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(wallet, ids);
    }
    // A type without a type group is of type group 1.
    let expected = newest_first(fixture, |_, t| {
        (t.kind().type_group(), t.kind().type_id()) == (1, 2)
    });
    assert_eq!(index(query(json!({"type": 2}))).await.0, expected);
    let expected = newest_first(fixture, |_, t| {
        (t.kind().type_group(), t.kind().type_id()) == (2, 2)
    });
    assert_eq!(
        index(query(json!({"type": 2, "typeGroup": 2}))).await.0,
        expected
    );
    // A memo pattern on the memo's bytes.
    let memos: Vec<String> = fixture
        .transactions()
        .iter()
        .filter_map(|(_, _, t)| t.data().memo.as_ref().map(|m| m.as_str().to_owned()))
        .collect();
    let memo = memos.iter().find(|m| m.len() > 4).unwrap();
    let part: String = memo.chars().skip(1).take(3).collect();
    let expected = newest_first(fixture, |_, t| {
        t.data()
            .memo
            .as_ref()
            .is_some_and(|m| m.as_str().contains(&part))
    });
    assert_eq!(
        index(query(json!({"memo": format!("%{part}%")}))).await.0,
        expected
    );
    // Heights as ranges, several values as one of them.
    let expected = newest_first(fixture, |b, _| (2..=40).contains(&b.height()));
    let range = json!({"blockHeight": {"from": 2, "to": 40}});
    assert_eq!(index(query(range)).await.0, expected);
    let expected = newest_first(fixture, |b, _| b.height() == 1 || b.height() == 177);
    let heights = json!({"blockHeight": [1, 177]});
    assert_eq!(index(query(heights)).await.1, expected.len() as u64);
    // An order on the address, which is no column: the reference's empty page.
    let (ids, total) = index(query(json!({"orderBy": ["address:asc"]}))).await;
    assert_eq!((ids.len(), total), (0, 0));
    // Only a burn has an amount, as in the reference's table: no other type matches an amount,
    // and theirs sort after the burns from the lowest.
    let burn = |t: &Transaction| match t.data().asset {
        Asset::Burn { amount } => Some(amount),
        _ => None,
    };
    let burns = newest_first(fixture, |_, t| burn(t).is_some_and(|amount| amount > 0));
    assert!(!burns.is_empty());
    assert_eq!(index(query(json!({"amount": 0}))).await.1, 0);
    let (ids, total) = index(query(json!({"amount": {"from": 1}}))).await;
    assert_eq!((ids, total), (burns.clone(), burns.len() as u64));
    let (ids, _) = index(query(json!({"orderBy": ["amount:asc"]}))).await;
    let mut lowest: Vec<String> = ids[..burns.len()].to_vec();
    lowest.sort();
    let mut expected = burns.clone();
    expected.sort();
    assert_eq!(lowest, expected);
}

#[tokio::test]
#[ignore = "integration"]
async fn a_transaction_shows_its_block_and_confirmations() {
    let fixture = fixture();
    for (block, sequence, transaction) in fixture.transactions().into_iter().step_by(7) {
        let data = show(&transaction.id().to_hex(), json!({"transform": true}))
            .await
            .unwrap();
        assert_eq!(data["blockId"], json!(block.id().to_hex()));
        assert_eq!(data["blockHeight"], json!(block.height()));
        assert_eq!(
            data["confirmations"],
            json!(fixture.tip() - block.height() + 1),
            "{sequence}"
        );
        assert_eq!(data["timestamp"]["epoch"], json!(block.header().timestamp));
    }
}
