//! The wallet listings over HTTP on the committed devnet chain, with the real pagination service
//! and the state's index order: `GET /wallets` and `GET /wallets/top` answer alike, the pages
//! and offsets cut the same order, and the criteria and orders that the reference refuses or
//! throws on answer 422 and 500.

mod snapshot;

use serde_json::Value;

use snapshot::{chain, get, serve_chain};

fn addresses(body: &Value) -> Vec<String> {
    let data = body["data"].as_array().unwrap();
    data.iter()
        .map(|w| w["address"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
#[ignore = "integration"]
async fn the_wallet_listings_over_http() {
    let (address, _server) =
        serve_chain("listing-live", chain().pool(Vec::new()), Vec::new()).await;
    let first = get(address, "/api/wallets?limit=100").await;
    assert_eq!(first.status, 200);
    assert_eq!(first.body["meta"]["totalCount"], 350);
    assert_eq!(first.body["meta"]["pageCount"], 4);
    assert_eq!(
        get(address, "/api/wallets/top?limit=100").await.body["data"],
        first.body["data"]
    );
    // The pages cut one order; an offset replaces the page.
    let mut walked = Vec::new();
    for page in 1..=4 {
        walked.extend(addresses(
            &get(address, &format!("/api/wallets?limit=100&page={page}"))
                .await
                .body,
        ));
    }
    assert_eq!(walked.len(), 350);
    assert_eq!(walked[..100], addresses(&first.body)[..]);
    let offset = get(address, "/api/wallets?limit=10&offset=95&page=7").await;
    assert_eq!(addresses(&offset.body), walked[95..105]);
    let beyond = get(address, "/api/wallets?limit=100&page=5").await;
    assert_eq!(
        (beyond.status, beyond.body["data"].as_array().unwrap().len()),
        (200, 0)
    );
    // The order of the given property, ties in the index order.
    let ranked = get(
        address,
        "/api/wallets?orderBy=attributes.delegate.rank:asc&limit=3",
    )
    .await;
    let ranks: Vec<u64> = ranked.body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["attributes"]["delegate"]["rank"].as_u64().unwrap())
        .collect();
    assert_eq!(ranks, [1, 2, 3]);
    let found = get(address, "/api/wallets?address=dDneg%25&publicKey=02b85b%25").await;
    assert_eq!(found.body["meta"]["totalCount"], 1);
    // What the reference refuses and what it throws on.
    assert_eq!(get(address, "/api/wallets?limit=101").await.status, 422);
    assert_eq!(
        get(address, "/api/wallets?orderBy=nothing").await.status,
        422
    );
    assert_eq!(
        get(address, "/api/wallets?attributes.delegate.username.from=a")
            .await
            .status,
        500
    );
    assert_eq!(
        get(
            address,
            "/api/wallets?attributes.delegate.username.from=a&page=1"
        )
        .await
        .status,
        500
    );
}
