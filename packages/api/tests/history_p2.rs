//! The history routes that the store's ready readers answer, over the committed chain's store:
//! the delegates of a round, the first block, the votes and a wallet's sent, received and vote
//! transactions. They read the round, block and transaction readers only, so they run in every
//! build.

mod history_fixture;

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use heartwood_api::controllers::{blocks, delegate_history, rounds, votes, wallets};
use heartwood_api::{ApiError, ApiReply, RouteId};
use heartwood_crypto::{Address, KeyPair, Transaction};
use serde_json::{Value, json};

use history_fixture::{Fixture, fixture, request};

fn is_vote(transaction: &Transaction) -> bool {
    let kind = transaction.kind();
    matches!((kind.type_group(), kind.type_id()), (1, 3) | (2, 2))
}

fn sender(fixture: &Fixture, transaction: &Transaction) -> String {
    let network = fixture.milestones().at(1);
    transaction.data().sender_address(network).to_base58()
}

/// A validated query of a transaction route with `criteria` over the defaults.
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

fn ids(reply: &ApiReply) -> Vec<String> {
    reply.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect()
}

fn total(reply: &ApiReply) -> u64 {
    reply.body["meta"]["totalCount"].as_u64().unwrap()
}

/// The ids of the chain's transactions that `keep` keeps, newest first.
fn newest_first(fixture: &Fixture, keep: impl Fn(&Transaction) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = fixture
        .transactions()
        .into_iter()
        .filter(|(_, _, transaction)| keep(transaction))
        .map(|(_, _, transaction)| transaction.id().to_hex())
        .collect();
    ids.reverse();
    ids
}

fn not_found(message: &str) -> Value {
    json!({"statusCode": 404, "error": "Not Found", "message": message})
}

/// The reference's stored rounds: each round's delegates and their balances, in the route's
/// order.
fn reference_rounds() -> BTreeMap<u64, Vec<Value>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/difftest/corpus/txgen-one-node/reference/rounds.jsonl");
    let mut rounds: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    for line in fs::read_to_string(path).unwrap().lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        rounds
            .entry(row["round"].as_u64().unwrap())
            .or_default()
            .push(json!({"publicKey": row["publicKey"], "votes": row["balance"]}));
    }
    for delegates in rounds.values_mut() {
        delegates.sort_by_key(|d| {
            let votes: u64 = d["votes"].as_str().unwrap().parse().unwrap();
            (Reverse(votes), d["publicKey"].as_str().unwrap().to_owned())
        });
    }
    rounds
}

async fn round(id: &str) -> Result<ApiReply, ApiError> {
    let request = request(RouteId::RoundsDelegates, &[("id", id)], json!({}));
    rounds::delegates(fixture().ctx.clone(), request).await
}

#[tokio::test]
async fn a_round_lists_its_delegates_with_the_reference_balances() {
    let expected = reference_rounds();
    assert!(expected.len() >= 5);
    for (number, delegates) in &expected {
        let reply = round(&number.to_string()).await.unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, json!({"data": delegates}), "round {number}");
    }
    // The later rounds carry votes, as strings.
    let last = &expected[expected.keys().next_back().unwrap()];
    assert!(last.iter().any(|delegate| delegate["votes"] != "0"));
}

#[tokio::test]
async fn a_round_the_chain_has_not_reached_is_not_found() {
    let last = *reference_rounds().keys().next_back().unwrap();
    for id in [(last + 1).to_string(), "9007199254740991".to_owned()] {
        let error = round(&id).await.unwrap_err();
        assert_eq!(error.body(), not_found("Round not found"), "{id}");
    }
    let error = round("0").await.unwrap_err();
    assert_eq!(error.status, 404);
}

async fn first(query: Value) -> Value {
    let request = request(RouteId::BlocksFirst, &[], query);
    let reply = blocks::first(fixture().ctx.clone(), request).await.unwrap();
    reply.body["data"].clone()
}

async fn block_at_one(query: Value) -> Value {
    let request = request(RouteId::BlocksShow, &[("id", "1")], query);
    let reply = blocks::show(fixture().ctx.clone(), request).await.unwrap();
    reply.body["data"].clone()
}

#[tokio::test]
async fn the_first_block_is_the_block_at_height_one() {
    let fixture = fixture();
    let raw = first(json!({"transform": false})).await;
    assert_eq!(raw["height"], json!(1));
    assert_eq!(raw["id"], json!(fixture.genesis.id().to_hex()));
    assert_eq!(raw, block_at_one(json!({"transform": false})).await);
}

#[tokio::test]
async fn the_first_block_is_transformed_by_default() {
    let fixture = fixture();
    let data = first(json!({"transform": true})).await;
    assert_eq!(data["height"], json!(1));
    assert_eq!(data["id"], json!(fixture.genesis.id().to_hex()));
    assert_eq!(data["confirmations"], json!(fixture.tip() - 1));
    assert_eq!(data, block_at_one(json!({"transform": true})).await);
    assert!(data["forged"]["amount"].is_string());
}

async fn vote(id: &str, transform: bool) -> Result<ApiReply, ApiError> {
    let request = request(
        RouteId::VotesShow,
        &[("id", id)],
        json!({"transform": transform}),
    );
    votes::show(fixture().ctx.clone(), request).await
}

#[tokio::test]
async fn a_vote_shows_raw_and_a_transaction_of_another_kind_is_not_found() {
    let fixture = fixture();
    let mut seen = Vec::new();
    for (block, sequence, transaction) in fixture.transactions() {
        let id = transaction.id().to_hex();
        if !is_vote(transaction) {
            let error = vote(&id, false).await.unwrap_err();
            assert_eq!(error.body(), not_found("Vote not found"), "{id}");
            continue;
        }
        let data = vote(&id, false).await.unwrap().body["data"].clone();
        assert_eq!(data["id"], json!(id));
        assert_eq!(data["blockHeight"], json!(block.height()));
        assert_eq!(data["sequence"], json!(sequence));
        assert_eq!(
            data["typeGroup"],
            json!(transaction.kind().type_group()),
            "{id}"
        );
        seen.push((
            transaction.kind().type_group(),
            transaction.kind().type_id(),
        ));
    }
    // The chain's votes are the Solar kind.
    assert!(seen.len() > 10 && seen.iter().all(|kind| *kind == (2, 2)));
    for id in ["ab".repeat(32), "not an id".to_owned(), String::new()] {
        let error = vote(&id, false).await.unwrap_err();
        assert_eq!(error.body(), not_found("Vote not found"), "{id:?}");
    }
    let (_, _, any) = fixture
        .transactions()
        .into_iter()
        .find(|t| is_vote(t.2))
        .unwrap();
    let upper = any.id().to_hex().to_uppercase();
    assert_eq!(vote(&upper, false).await.unwrap_err().status, 404);
}

#[tokio::test]
async fn a_transformed_vote_has_no_timestamp_and_no_confirmations() {
    let fixture = fixture();
    let mut shown = 0;
    for (block, _, transaction) in fixture.transactions() {
        if !is_vote(transaction) {
            continue;
        }
        let data = vote(&transaction.id().to_hex(), true).await.unwrap().body["data"].clone();
        assert_eq!(data["blockId"], json!(block.id().to_hex()));
        assert_eq!(data["blockHeight"], json!(block.height()));
        assert_eq!(data["confirmations"], json!(0));
        assert!(data.get("timestamp").is_none());
        let key = transaction.data().sender_public_key.to_hex();
        assert_eq!(data["senderPublicKey"], json!(key));
        shown += 1;
    }
    assert!(shown >= 2);
}

async fn votes_index(query: Value) -> ApiReply {
    let request = request(RouteId::VotesIndex, &[], query);
    votes::index(fixture().ctx.clone(), request).await.unwrap()
}

#[tokio::test]
async fn the_votes_list_every_vote_of_both_kinds_newest_first() {
    let fixture = fixture();
    let all = newest_first(fixture, is_vote);
    assert!(all.len() >= 2);
    let reply = votes_index(query(json!({}))).await;
    assert_eq!(
        (ids(&reply), total(&reply)),
        (all.clone(), all.len() as u64)
    );
    // The transactions' resource: no timestamp, no confirmations.
    for data in reply.body["data"].as_array().unwrap() {
        assert_eq!(data["confirmations"], json!(0));
        assert!(data.get("timestamp").is_none());
    }
    // The query's own kind is replaced by the vote kinds.
    for given in [
        json!({"typeGroup": 1, "type": 0}),
        json!({"typeGroup": 2, "type": 2}),
        json!({"type": 7}),
    ] {
        assert_eq!(ids(&votes_index(query(given)).await), all);
    }
    // The raw models.
    let raw = votes_index(query(json!({"transform": false}))).await;
    assert_eq!(ids(&raw), all);
    assert!(raw.body["data"][0].get("blockHeight").is_some());
    assert!(!raw.body["meta"]["totalCountIsEstimate"].as_bool().unwrap());
}

#[tokio::test]
async fn the_votes_narrow_with_each_criterion() {
    let fixture = fixture();
    let (block, _, one) = fixture
        .transactions()
        .into_iter()
        .rev()
        .find(|t| is_vote(t.2))
        .unwrap();
    let voter = sender(fixture, one);
    let expected = newest_first(fixture, |t| is_vote(t) && sender(fixture, t) == voter);
    let reply = votes_index(query(json!({"senderId": voter}))).await;
    assert_eq!(ids(&reply), expected);
    let key = one.data().sender_public_key.to_hex();
    let expected = newest_first(fixture, |t| {
        is_vote(t) && t.data().sender_public_key.to_hex() == key
    });
    let reply = votes_index(query(json!({"senderPublicKey": key}))).await;
    assert_eq!(ids(&reply), expected);
    let reply = votes_index(query(json!({"id": one.id().to_hex()}))).await;
    assert_eq!(ids(&reply), [one.id().to_hex()]);
    let height = json!({"blockHeight": {"from": block.height(), "to": block.height()}});
    let reply = votes_index(query(height)).await;
    assert!(ids(&reply).contains(&one.id().to_hex()));
    let reply = votes_index(query(json!({"blockHeight": 0}))).await;
    assert_eq!(total(&reply), 0);
    // A page beyond the votes is empty, the count intact.
    let all = newest_first(fixture, is_vote);
    let reply = votes_index(query(json!({"page": 2, "limit": 1}))).await;
    assert_eq!(
        (ids(&reply), total(&reply)),
        (all[1..2].to_vec(), all.len() as u64)
    );
    // Ordering by the fee sorts the votes by their fee.
    let reply = votes_index(query(json!({"orderBy": ["fee:asc"]}))).await;
    let fees: Vec<u64> = reply.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["fee"].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(fees.windows(2).all(|pair| pair[0] <= pair[1]));
}

async fn wallet_route(route: RouteId, id: &str, query: Value) -> Result<ApiReply, ApiError> {
    let request = request(route, &[("id", id)], query);
    let ctx = fixture().ctx.clone();
    match route {
        RouteId::WalletsTransactionsSent => wallets::sent(ctx, request).await,
        RouteId::WalletsTransactionsReceived => wallets::received(ctx, request).await,
        _ => wallets::votes(ctx, request).await,
    }
}

#[tokio::test]
async fn a_wallets_sent_transactions_are_the_ones_its_address_sent() {
    let fixture = fixture();
    let route = RouteId::WalletsTransactionsSent;
    // The address that sent most, by address, by public key and by delegate name.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, _, transaction) in fixture.transactions() {
        *counts.entry(sender(fixture, transaction)).or_default() += 1;
    }
    let (address, _) = counts.iter().max_by_key(|(_, count)| **count).unwrap();
    let expected = newest_first(fixture, |t| sender(fixture, t) == *address);
    assert!(expected.len() > 2);
    let by_address = wallet_route(route, address, query(json!({"limit": 1000 })));
    let reply = by_address.await.unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(ids(&reply), expected[..expected.len().min(1000)]);
    assert_eq!(total(&reply), expected.len() as u64);
    // The block-joined resource carries the block's timestamp and confirmations.
    for data in reply.body["data"].as_array().unwrap() {
        assert!(data["timestamp"]["epoch"].is_number());
        assert!(data["confirmations"].as_u64().unwrap() >= 1);
    }
    // A `senderId` of the query is replaced by the wallet's address.
    let other = counts.keys().find(|other| *other != address).unwrap();
    let given = query(json!({"senderId": other}));
    let reply = wallet_route(route, address, given).await.unwrap();
    assert_eq!(ids(&reply), expected[..expected.len().min(100)]);
    // Another criterion narrows it.
    let one = &expected[expected.len() / 2];
    let reply = wallet_route(route, address, query_with_id(one))
        .await
        .unwrap();
    assert_eq!(ids(&reply), std::slice::from_ref(one));
    // The raw models.
    let raw = query(json!({"transform": false}));
    let reply = wallet_route(route, address, raw).await.unwrap();
    assert_eq!(ids(&reply), expected[..expected.len().min(100)]);
    assert!(reply.body["data"][0].get("timestamp").is_none());
}

fn query_with_id(id: &str) -> Value {
    query(json!({"id": id}))
}

/// The address of a wallet that was paid and never sent.
fn only_paid(fixture: &Fixture) -> String {
    let senders: Vec<String> = fixture
        .transactions()
        .iter()
        .map(|(_, _, t)| sender(fixture, t))
        .collect();
    fixture
        .transactions()
        .iter()
        .filter_map(|(_, _, t)| match &t.data().asset {
            heartwood_crypto::transactions::types::Asset::Transfer { transfers } => {
                Some(transfers.iter().map(|item| item.recipient.to_base58()))
            }
            _ => None,
        })
        .flatten()
        .find(|address| !senders.contains(address))
        .unwrap()
}

#[tokio::test]
async fn a_wallet_without_a_public_key_has_an_empty_page_of_sent_and_of_votes() {
    let fixture = fixture();
    let paid = only_paid(fixture);
    for route in [RouteId::WalletsTransactionsSent, RouteId::WalletsVotes] {
        let reply = wallet_route(route, &paid, query(json!({"page": 3, "limit": 7})))
            .await
            .unwrap();
        assert_eq!(reply.status, 200);
        let meta = &reply.body["meta"];
        assert_eq!(meta["totalCount"], json!(0));
        assert_eq!(meta["totalCountIsEstimate"], json!(false));
        assert_eq!(meta["count"], json!(0));
        assert_eq!(meta["pageCount"], json!(1));
        assert_eq!(reply.body["data"], json!([]));
    }
    // What it received is listed: the empty page is not the rule for the received.
    let received = newest_first(fixture, |t| match &t.data().asset {
        heartwood_crypto::transactions::types::Asset::Transfer { transfers } => transfers
            .iter()
            .any(|item| item.recipient.to_base58() == paid),
        _ => false,
    });
    let reply = wallet_route(
        RouteId::WalletsTransactionsReceived,
        &paid,
        query(json!({})),
    )
    .await
    .unwrap();
    assert_eq!(ids(&reply), received);
    assert_eq!(total(&reply), received.len() as u64);
}

#[tokio::test]
async fn a_wallets_votes_are_the_vote_transactions_it_sent() {
    let fixture = fixture();
    let route = RouteId::WalletsVotes;
    let (_, _, one) = fixture
        .transactions()
        .into_iter()
        .find(|t| is_vote(t.2))
        .unwrap();
    let voter = sender(fixture, one);
    let expected = newest_first(fixture, |t| is_vote(t) && sender(fixture, t) == voter);
    let reply = wallet_route(route, &voter, query(json!({}))).await.unwrap();
    assert_eq!(ids(&reply), expected);
    assert_eq!(total(&reply), expected.len() as u64);
    for data in reply.body["data"].as_array().unwrap() {
        assert!(data["timestamp"]["epoch"].is_number());
        assert!(data["confirmations"].as_u64().unwrap() >= 1);
    }
    // The route's kinds and sender replace the query's.
    let given = query(json!({"typeGroup": 1, "type": 0, "senderId": "x".repeat(34)}));
    let reply = wallet_route(route, &voter, given).await.unwrap();
    assert_eq!(ids(&reply), expected);
    let raw = wallet_route(route, &voter, query(json!({"transform": false})))
        .await
        .unwrap();
    assert_eq!(ids(&raw), expected);
    assert!(raw.body["data"][0].get("timestamp").is_none());
}

#[tokio::test]
async fn a_wallets_received_transactions_are_the_transfers_paying_it() {
    let fixture = fixture();
    let route = RouteId::WalletsTransactionsReceived;
    let paid = only_paid(fixture);
    let expected = newest_first(fixture, |t| match &t.data().asset {
        heartwood_crypto::transactions::types::Asset::Transfer { transfers } => transfers
            .iter()
            .any(|item| item.recipient.to_base58() == paid),
        _ => false,
    });
    // A `recipientId` of the query is replaced by the wallet's address.
    let other = sender(fixture, fixture.transactions()[3].2);
    let reply = wallet_route(route, &paid, query(json!({"recipientId": other})))
        .await
        .unwrap();
    assert_eq!(ids(&reply), expected);
    let raw = wallet_route(route, &paid, query(json!({"transform": false})))
        .await
        .unwrap();
    assert_eq!(ids(&raw), expected);
    assert!(!expected.is_empty());
}

#[tokio::test]
async fn an_unknown_wallet_is_refused_or_has_nothing() {
    let fixture = fixture();
    for route in [
        RouteId::WalletsTransactionsSent,
        RouteId::WalletsTransactionsReceived,
        RouteId::WalletsVotes,
    ] {
        // 34 characters that are no address: 422; any other text: 404.
        let error = wallet_route(
            route,
            "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjZ",
            query(json!({})),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (error.status, error.message.as_str()),
            (422, "Wallet not valid")
        );
        let error = wallet_route(route, "nobody", query(json!({})))
            .await
            .unwrap_err();
        assert_eq!(error.body(), not_found("Wallet not found"));
        // An address the chain never named is a wallet with no key and no transactions.
        let unknown = Address::from_public_key(
            &KeyPair::from_passphrase("never used on this chain")
                .unwrap()
                .public_key()
                .clone(),
            fixture.ctx.state().wallets.network(),
        )
        .to_base58();
        let reply = wallet_route(route, &unknown, query(json!({})))
            .await
            .unwrap();
        assert_eq!((ids(&reply).len(), total(&reply)), (0, 0));
    }
}

/// `base` with `criteria` over it.
fn with(mut base: Value, criteria: Value) -> Value {
    for (key, value) in criteria.as_object().unwrap() {
        base[key] = value.clone();
    }
    base
}

/// The data, the count and the estimate flag of a page.
fn page_of(reply: &ApiReply) -> (Value, Value, Value) {
    let meta = &reply.body["meta"];
    (
        reply.body["data"].clone(),
        meta["totalCount"].clone(),
        meta["totalCountIsEstimate"].clone(),
    )
}

#[tokio::test]
async fn a_criterion_beyond_its_columns_type_gives_the_failed_listings_empty_page() {
    // The reference's store refuses a value beyond `integer` (`smallint` for the version), its
    // listing fails, and the answer is an empty page whose flag is the listing options' (none on
    // the raw block list, so `false`).
    let empty = (json!([]), json!(0), json!(false));
    let beyond = 2_147_483_648_u64;
    let blocks_query = json!({"page": 1, "limit": 100, "orderBy": "height:desc"});
    for criteria in [
        json!({"height": {"from": 1, "to": beyond}}),
        json!({"version": 32_768}),
        json!({"timestamp": [5, beyond]}),
        json!({"numberOfTransactions": {"to": beyond}}),
        json!({"payloadLength": beyond}),
    ] {
        for transform in [false, true] {
            let given = with(
                with(blocks_query.clone(), criteria.clone()),
                json!({"transform": transform}),
            );
            let asked = request(RouteId::BlocksIndex, &[], given);
            let reply = blocks::index(fixture().ctx.clone(), asked).await.unwrap();
            assert_eq!(page_of(&reply), empty, "{criteria} {transform}");
            let given = with(
                with(blocks_query.clone(), criteria.clone()),
                json!({"transform": transform}),
            );
            let asked = request(RouteId::DelegatesBlocks, &[("id", "genesis_1")], given);
            let reply = delegate_history::blocks(fixture().ctx.clone(), asked)
                .await
                .unwrap();
            assert_eq!(page_of(&reply), empty, "{criteria} {transform}");
        }
    }
    let missed_query = json!({"page": 1, "limit": 100, "orderBy": "timestamp:desc"});
    for criteria in [
        json!({"height": {"to": beyond}}),
        json!({"timestamp": beyond}),
        json!({"timestamp": {"to": beyond}}),
        json!({"timestamp": [1, beyond]}),
    ] {
        let given = with(missed_query.clone(), criteria.clone());
        let asked = request(RouteId::BlocksMissed, &[], given.clone());
        let reply = blocks::missed(fixture().ctx.clone(), asked).await.unwrap();
        assert_eq!(page_of(&reply), empty, "{criteria}");
        let asked = request(
            RouteId::DelegatesMissedBlocks,
            &[("id", "genesis_1")],
            given,
        );
        let reply = delegate_history::missed_blocks(fixture().ctx.clone(), asked)
            .await
            .unwrap();
        assert_eq!(page_of(&reply), empty, "{criteria}");
    }
}
