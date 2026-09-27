//! `GET /wallets/{id}` and `GET /wallets/{id}/transactions` over the committed devnet chain: every
//! wallet of the chain against the reference's REST state at the tip, the lookup by address,
//! public key and name, the zero wallet of an unknown address, and the two refusals.

mod snapshot;

use heartwood_api::controllers::wallets;
use heartwood_api::resources;
use serde_json::{Value, json};

use snapshot::chain;

/// The JSON text of `value`, keys in their order.
fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

/// Every wallet equals the reference's; its text too (keys in order), except a delegate whose
/// rank the reference forgot and set again, which moved the key last there.
#[test]
fn every_wallet_equals_the_reference_at_the_tip() {
    let chain = chain();
    let pool = chain.pool(Vec::new());
    let expected = chain.checkpoint["wallets"].as_array().unwrap();
    assert!(expected.len() > 300);
    let mut reordered = Vec::new();
    for reference in expected {
        let address = reference["address"].as_str().unwrap();
        let wallet = wallets::find(&chain.state, address).unwrap();
        let ours = resources::wallet(&wallet, &chain.state, &pool);
        assert_eq!(ours, *reference);
        if text(&ours) != text(reference) {
            reordered.push(reference["attributes"]["delegate"]["username"].clone());
        }
    }
    assert_eq!(reordered, [json!("genesis_53")]);
}

#[test]
fn a_genesis_delegate_keeps_the_reference_key_order() {
    let chain = chain();
    let pool = chain.pool(Vec::new());
    let reference = chain.reference_wallet("dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY");
    let wallet = wallets::find(&chain.state, "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY").unwrap();
    let ours = resources::wallet(&wallet, &chain.state, &pool);
    assert_eq!(text(&ours), text(reference));
}

#[test]
fn a_wallet_is_found_by_address_public_key_and_name() {
    let chain = chain();
    let address = "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY";
    let key = "02b85b15ff121c2408ea682a6f6bf1a1079bf42537cdeef9e039befa26671949df";
    for id in [address, key, "genesis_34"] {
        let wallet = wallets::find(&chain.state, id).unwrap();
        assert_eq!(wallet.address().to_base58(), address, "{id}");
    }
    // The key's index holds lowercase hex only.
    let upper = wallets::find(&chain.state, &key.to_uppercase()).unwrap_err();
    assert_eq!(upper.status, 404);
}

#[test]
fn an_unknown_valid_address_is_a_zero_wallet() {
    let chain = chain();
    let pool = chain.pool(Vec::new());
    // A valid address of the network that the chain never named.
    let unknown = heartwood_crypto::Address::from_public_key(
        &heartwood_crypto::KeyPair::from_passphrase("never used on this chain")
            .unwrap()
            .public_key()
            .clone(),
        chain.state.wallets.network(),
    )
    .to_base58();
    let wallet = wallets::find(&chain.state, &unknown).unwrap();
    let ours = resources::wallet(&wallet, &chain.state, &pool);
    let expected = json!({
        "address": unknown,
        "balance": "0",
        "nonce": "0",
        "attributes": {},
        "votingFor": {},
    });
    assert_eq!(ours, expected);
}

#[test]
fn an_unknown_id_is_422_at_34_characters_and_404_otherwise() {
    let chain = chain();
    // 34 characters, not an address of the network (a broken checksum).
    let refused = wallets::find(&chain.state, "dDnegkP1tnzssM15CAs3rq3KuwFqkikNjZ").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (422, "Wallet not valid")
    );
    for id in [
        "nobody",
        "02b85b15ff121c2408ea682a6f6bf1a1079bf42537cdeef9e039befa26671949de",
    ] {
        let missing = wallets::find(&chain.state, id).unwrap_err();
        assert_eq!(
            (missing.status, missing.message.as_str()),
            (404, "Wallet not found"),
            "{id}"
        );
    }
}

#[test]
fn a_delegate_version_comes_from_the_online_record_only() {
    let chain = chain();
    let wallet = wallets::find(&chain.state, "genesis_34").unwrap();
    let without = resources::wallet(&wallet, &chain.state, &{
        let mut pool = chain.pool(Vec::new());
        pool.online.clear();
        pool
    });
    assert!(without["attributes"]["delegate"].get("version").is_none());
    let with = resources::wallet(&wallet, &chain.state, &chain.pool(Vec::new()));
    assert_eq!(with["attributes"]["delegate"]["version"], "4.3.1");
}

#[tokio::test]
#[ignore = "integration"]
async fn the_wallet_routes_over_http() {
    let chain = chain();
    let (address, server) =
        snapshot::serve_chain("wallets", chain.pool(Vec::new()), Vec::new()).await;
    let shown = snapshot::get(address, "/api/wallets/genesis_34").await;
    let reference = chain.reference_wallet("dDnegkP1tnzssM15CAs3rq3KuwFqkikNjY");
    assert_eq!(text(&shown.body["data"]), text(reference));
    let refused = snapshot::get(address, "/api/wallets/dDnegkP1tnzssM15CAs3rq3KuwFqkikNjZ").await;
    assert_eq!(
        (refused.status, &refused.body["message"]),
        (422, &json!("Wallet not valid"))
    );
    let missing = snapshot::get(address, "/api/wallets/nobody").await;
    assert_eq!(
        (missing.status, &missing.body["message"]),
        (404, &json!("Wallet not found"))
    );
    // The wallet's history: its address is the criterion, and the links keep the query as given.
    let history = snapshot::get(address, "/api/wallets/genesis_34/transactions?limit=10").await;
    assert_eq!(history.status, 200);
    let links = ["self", "first", "last"].map(|key| history.body["meta"][key].to_string());
    for link in links {
        assert!(
            link.starts_with("\"/wallets/genesis_34/transactions?"),
            "{link}"
        );
        assert!(!link.contains("address="), "{link}");
    }
    server.stop().await;
}
