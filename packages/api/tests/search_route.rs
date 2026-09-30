//! `GET /blockchain/search/{id}` over the committed chain's store: the id scans are prefix scans,
//! and the input is lower-cased once, so an upper-case input finds what its lower-case form finds.
//! The route reads the store and the state, so its tests run with the whole crate
//! (`integration`). `GET /blockchain` is checked over the chain's tip with totals given.

mod history_fixture;

use std::sync::Arc;

use heartwood_api::controllers::blockchain;
use heartwood_api::{ApiContext, ApiError, ChainTotals, Productivity, ProductivityTable, RouteId};
use heartwood_crypto::Block;
use heartwood_crypto::transactions::types::Asset;
use serde_json::{Value, json};

use history_fixture::{fixture, request};

/// The `data` of the search for `id`.
async fn search(id: &str) -> Value {
    let request = request(RouteId::BlockchainSearch, &[("id", id)], json!({}));
    let reply = blockchain::search(fixture().ctx.clone(), request)
        .await
        .unwrap();
    assert_eq!(reply.status, 200, "{id}");
    reply.body["data"].clone()
}

/// The list at `key` of the search's data.
fn list<'a>(data: &'a Value, key: &str) -> &'a [Value] {
    data[key].as_array().unwrap()
}

#[tokio::test]
#[ignore = "integration"]
async fn a_block_id_prefix_finds_the_block_in_either_case() {
    let block = fixture().block(100);
    let id = block.id().to_hex();
    for length in [21, 40, 64] {
        let prefix = &id[..length];
        let lower = search(prefix).await;
        let blocks = list(&lower, "blocks");
        assert_eq!(blocks.len(), 1, "{prefix}");
        assert_eq!(blocks[0]["id"], json!(id));
        assert_eq!(blocks[0]["height"], json!(100));
        assert_eq!(blocks[0]["timestamp"], json!(block.header().timestamp));
        assert_eq!(
            blocks[0]["transactions"],
            json!(block.header().number_of_transactions)
        );
        assert!(list(&lower, "transactions").is_empty());
        assert_eq!(search(&prefix.to_uppercase()).await, lower, "{prefix}");
    }
}

#[tokio::test]
#[ignore = "integration"]
async fn a_transaction_id_prefix_finds_the_transaction_in_either_case() {
    let fixture = fixture();
    let (_, _, transfer) = fixture
        .transactions()
        .into_iter()
        .find(|(block, _, transaction)| {
            block.height() > 1 && matches!(transaction.data().asset, Asset::Transfer { .. })
        })
        .unwrap();
    let id = transfer.id().to_hex();
    let lower = search(&id[..24]).await;
    let found = list(&lower, "transactions");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["id"], json!(id));
    assert_eq!(found[0]["typeGroup"], json!(1));
    assert_eq!(found[0]["type"], json!(6));
    assert!(found[0]["sender"].is_string());
    assert!(list(&lower, "blocks").is_empty());
    assert_eq!(search(&id[..24].to_uppercase()).await, lower);
}

#[tokio::test]
#[ignore = "integration"]
async fn an_input_below_the_scan_length_scans_no_ids() {
    let id = fixture().block(100).id().to_hex();
    for input in [&id[..20], &id[..1], "zzzz"] {
        let data = search(input).await;
        assert!(list(&data, "blocks").is_empty(), "{input}");
        assert!(list(&data, "transactions").is_empty(), "{input}");
    }
    // Longer than the scans take, the ids match nothing.
    let long = format!("{id}{id}");
    let data = search(&long[..65]).await;
    assert!(list(&data, "blocks").is_empty());
    assert!(list(&data, "transactions").is_empty());
}

#[tokio::test]
#[ignore = "integration"]
async fn a_delegate_name_finds_its_wallet_in_either_case() {
    let lower = search("genesis_1").await;
    let wallets = list(&lower, "wallets");
    assert!(!wallets.is_empty());
    assert_eq!(wallets[0]["delegate"]["username"], "genesis_1");
    assert_eq!(search("GENESIS_1").await, lower);
    let address = wallets[0]["address"].as_str().unwrap();
    let by_address = search(&address.to_uppercase()).await;
    assert!(
        list(&by_address, "wallets")
            .iter()
            .any(|wallet| wallet["address"] == json!(address))
    );
    assert!(list(&search("no_such_name").await, "wallets").is_empty());
}

/// A node whose chain totals are fixed.
struct FixedTotals;

impl Productivity for FixedTotals {
    fn table(&self, _tip: &Block) -> Result<Arc<ProductivityTable>, ApiError> {
        Err(ApiError::internal())
    }

    fn totals(&self, _tip: &Block) -> Result<ChainTotals, ApiError> {
        Ok(ChainTotals {
            burned_fees: 12_345,
            burned_transactions: u128::from(u64::MAX) + 1,
            supply: 7_000_000_000_000_000_000,
        })
    }
}

#[tokio::test]
async fn the_chain_summary_writes_the_tip_and_the_totals_as_text() {
    let fixture = fixture();
    let base = &fixture.ctx;
    let ctx = Arc::new(ApiContext {
        state: base.state.clone(),
        pool: base.pool.clone(),
        status: base.status.clone(),
        peers: Arc::clone(&base.peers),
        history: Arc::clone(&base.history),
        submit: Arc::clone(&base.submit),
        productivity: Arc::new(FixedTotals),
        node: Arc::clone(&base.node),
        config: Arc::clone(&base.config),
    });
    let request = request(RouteId::BlockchainIndex, &[], json!({}));
    let reply = blockchain::index(ctx, request).await.unwrap();
    assert_eq!(reply.status, 200);
    let tip = fixture.block(fixture.tip());
    let expected = json!({
        "data": {
            "block": { "height": fixture.tip(), "id": tip.id().to_hex() },
            "burned": {
                "fees": "12345",
                "transactions": "18446744073709551616",
                "total": "18446744073709563961",
            },
            "supply": "7000000000000000000",
        }
    });
    // The keys in the reference's order too.
    assert_eq!(
        serde_json::to_string(&reply.body).unwrap(),
        serde_json::to_string(&expected).unwrap()
    );
    // Without totals, the route fails as the reference's repository does.
    let error = blockchain::index(
        Arc::clone(base),
        history_fixture::request(RouteId::BlockchainIndex, &[], json!({})),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, 500);
}
