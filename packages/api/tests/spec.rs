//! The OpenAPI document as a node serves it: the reference's document with its boot patches, for
//! two network configurations, cut to the routes and types this node carries.

use heartwood_api::RouteId;
use heartwood_api::context::Method;
use heartwood_api::spec::{Patches, document, transaction_schemas};
use serde_json::{Value, json};

fn patches(pub_key_hash: u8, max_transactions: usize, lookback: i64) -> Patches {
    Patches {
        pub_key_hash,
        max_transactions_per_request: max_transactions,
        base_path: "/api".to_owned(),
        version: "4.3.1-heartwood.0.1.0".to_owned(),
        missed_blocks_lookback: lookback,
    }
}

/// A path with each `{name}` written `{}`, so that the document's parameter names do not matter.
fn shape(path: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for character in path.chars() {
        match character {
            '{' => {
                inside = true;
                out.push_str("{}");
            }
            '}' => inside = false,
            _ if !inside => out.push(character),
            _ => {}
        }
    }
    out
}

fn schemas(doc: &Value) -> &Value {
    &doc["components"]["schemas"]
}

#[test]
fn the_default_network_writes_its_character_and_the_default_options() {
    // Address version 90 starts its addresses with `d`.
    let doc = document(&patches(90, 40, 604_800)).unwrap();
    let schemas = schemas(&doc);
    let address = schemas["address"]["pattern"].as_str().unwrap();
    assert!(address.starts_with("^d("), "{address}");
    assert_eq!(
        schemas["recipientId"]["pattern"],
        schemas["address"]["pattern"]
    );
    assert!(
        schemas["walletIdentifier"]["pattern"]
            .as_str()
            .unwrap()
            .starts_with("^d(")
    );
    let dummy = schemas["recipientId"]["example"].as_str().unwrap();
    assert!(dummy.starts_with('d'), "{dummy}");
    assert_eq!(
        schemas["transaction"]["properties"]["asset"]["example"]["transfers"][0]["recipientId"],
        dummy
    );
    assert_eq!(
        schemas["transactions"]["properties"]["transactions"]["maxItems"],
        40
    );
    assert_eq!(
        doc["servers"].as_array().unwrap().last().unwrap(),
        &json!({ "url": "/api" })
    );
    assert_eq!(doc["info"]["version"], "4.3.1-heartwood.0.1.0");
    for route in ["/blocks/missed", "/delegates/{identifier}/blocks/missed"] {
        let summary = doc["paths"][route]["get"]["summary"].as_str().unwrap();
        assert!(summary.contains(" 7 "), "{summary}");
        assert!(!summary.contains('X'), "{summary}");
    }
}

#[test]
fn another_network_writes_its_own_character_and_options() {
    let doc = document(&patches(30, 150, 172_800)).unwrap();
    let schemas = schemas(&doc);
    let address = schemas["address"]["pattern"].as_str().unwrap();
    assert!(address.starts_with("^D("), "{address}");
    assert!(
        schemas["walletIdentifier"]["pattern"]
            .as_str()
            .unwrap()
            .starts_with("^D(")
    );
    assert!(
        schemas["recipientId"]["example"]
            .as_str()
            .unwrap()
            .starts_with('D')
    );
    assert_eq!(
        schemas["transactions"]["properties"]["transactions"]["maxItems"],
        150
    );
    let summary = doc["paths"]["/blocks/missed"]["get"]["summary"]
        .as_str()
        .unwrap();
    assert!(summary.contains(" 2 "), "{summary}");
}

#[test]
fn the_document_lists_the_kept_types_and_versions_only() {
    let doc = document(&patches(90, 40, 604_800)).unwrap();
    let schemas = schemas(&doc);
    // The order of a node's handler registry, which is the order its service provider binds the
    // handlers in.
    assert_eq!(schemas["transactionTypes"]["enum"], json!([1, 2, 6, 7, 0]));
    assert_eq!(schemas["transactionTypeGroups"]["enum"], json!([1, 2]));
    assert_eq!(schemas["transactionVersions"]["enum"], json!([2, 3]));
}

#[test]
fn the_document_has_no_lock_operation() {
    let doc = document(&patches(90, 40, 604_800)).unwrap();
    let paths = doc["paths"].as_object().unwrap();
    assert!(paths.keys().all(|path| !path.starts_with("/locks")));
    assert!(!paths.contains_key("/wallets/{identifier}/locks"));
    assert!(paths.contains_key("/blockchain/search/{id}"));
}

#[test]
fn every_document_route_is_a_route_of_the_node() {
    let doc = document(&patches(90, 40, 604_800)).unwrap();
    let served: Vec<(String, &str)> = RouteId::ALL
        .iter()
        .map(|route| {
            let method = match route.method() {
                Method::Get => "get",
                _ => "post",
            };
            (shape(route.path()), method)
        })
        .collect();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            assert!(
                served
                    .iter()
                    .any(|(served, m)| *served == shape(path) && m == method),
                "{method} {path} is documented but not served"
            );
        }
    }
}

#[test]
fn the_transaction_schemas_are_those_of_the_kept_types() {
    let reply = transaction_schemas().unwrap();
    let data = &reply.body["data"];
    let groups: Vec<&String> = data.as_object().unwrap().keys().collect();
    assert_eq!(groups, ["1", "2"]);
    let mut kept: Vec<(&str, &str)> = Vec::new();
    for (group, types) in data.as_object().unwrap() {
        for kind in types.as_object().unwrap().keys() {
            kept.push((group, kind));
        }
    }
    kept.sort_unstable();
    assert_eq!(
        kept,
        [
            ("1", "1"),
            ("1", "2"),
            ("1", "6"),
            ("1", "7"),
            ("2", "0"),
            ("2", "2")
        ]
    );
}
