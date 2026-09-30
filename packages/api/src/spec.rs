//! The API's documents that are data: `GET /api.json`, the OpenAPI document with the reference's
//! patches at its start (the network's address character, the pool's and the blockchain's
//! options, the version), listing the kept routes and the kept transaction types; and
//! `GET /transactions/schemas`' answer, the schema properties of the kept types.
//!
//! The OpenAPI document derives from the reference implementation's, which descends from ARK
//! Core's (MIT License); the NOTICE file carries that notice.
//! Both files are in `assets/`, parsed once.

use std::sync::OnceLock;

use heartwood_crypto::identities::{Address, KeyPair};
use heartwood_crypto::utils::js;
use serde_json::{Value, json};

use crate::context::{ApiContext, ApiReply};
use crate::errors::ApiError;
use crate::resources::fee_statistics::REGISTERED;

/// The OpenAPI document as the assets hold it, before the patches.
const DOCUMENT: &str = include_str!("../assets/api.json");

/// The schema properties of the kept types, by type group and type.
const SCHEMAS: &str = include_str!("../assets/transaction-schemas.json");

/// What the document is patched with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patches {
    /// The network's address version byte.
    pub pub_key_hash: u8,
    /// The pool's `maxTransactionsPerRequest`.
    pub max_transactions_per_request: usize,
    /// The API's base path, listed as the server.
    pub base_path: String,
    /// The node's version.
    pub version: String,
    /// The blockchain's `missedBlocksLookback`, in seconds.
    pub missed_blocks_lookback: i64,
}

/// The document of `context`'s node: the assets' document with [`patched`]. A document that does
/// not parse or lacks a patched key is an internal error.
pub fn answer(context: &ApiContext) -> ApiReply {
    let patches = Patches {
        pub_key_hash: context.node.crypto.network.pub_key_hash,
        max_transactions_per_request: context.pool().options.max_transactions_per_request,
        base_path: context.config.base_path.clone(),
        version: context.node.version.clone(),
        missed_blocks_lookback: context.config.missed_blocks_lookback,
    };
    match document(&patches) {
        Some(document) => ApiReply::ok(document),
        None => internal(),
    }
}

/// The body of `GET /transactions/schemas`: the schemas of the kept types, which are all
/// activated whatever the height.
pub fn transaction_schemas() -> Result<ApiReply, ApiError> {
    static PARSED: OnceLock<Option<Value>> = OnceLock::new();
    PARSED
        .get_or_init(|| serde_json::from_str(SCHEMAS).ok())
        .clone()
        .map(ApiReply::data)
        .ok_or_else(ApiError::internal)
}

fn internal() -> ApiReply {
    let error = ApiError::internal();
    ApiReply {
        status: error.status,
        body: error.body(),
    }
}

/// The assets' document, parsed once.
fn base() -> Option<&'static Value> {
    static PARSED: OnceLock<Option<Value>> = OnceLock::new();
    PARSED
        .get_or_init(|| serde_json::from_str(DOCUMENT).ok())
        .as_ref()
}

/// The assets' document with the reference's boot patches, or `None` when it does not parse or
/// lacks a key they set into.
pub fn document(patches: &Patches) -> Option<Value> {
    base().and_then(|document| patched(document, patches))
}

/// `document` with the reference's boot patches; `None` when a key they set into is not there.
pub fn patched(document: &Value, patches: &Patches) -> Option<Value> {
    let mut doc = document.clone();
    let dummy = dummy_address(patches.pub_key_hash)?;
    let character = dummy.get(..1)?;
    doc.pointer_mut("/servers")?
        .as_array_mut()?
        .push(json!({ "url": patches.base_path }));
    put(&mut doc, "/info", "version", json!(patches.version))?;
    let schemas = "/components/schemas";
    let address = pattern(&doc, "/components/schemas/address/pattern", character)?;
    put(
        &mut doc,
        &format!("{schemas}/address"),
        "pattern",
        json!(address),
    )?;
    put(
        &mut doc,
        &format!("{schemas}/recipientId"),
        "pattern",
        json!(address),
    )?;
    put(
        &mut doc,
        &format!("{schemas}/recipientId"),
        "example",
        json!(dummy),
    )?;
    let example = format!("{schemas}/transaction/properties/asset/example/transfers/0");
    put(&mut doc, &example, "recipientId", json!(dummy))?;
    let wallet = pattern(
        &doc,
        "/components/schemas/walletIdentifier/pattern",
        character,
    )?;
    put(
        &mut doc,
        &format!("{schemas}/walletIdentifier"),
        "pattern",
        json!(wallet),
    )?;
    let items = format!("{schemas}/transactions/properties/transactions");
    put(
        &mut doc,
        &items,
        "maxItems",
        json!(patches.max_transactions_per_request),
    )?;

    let (mut types, mut groups): (Vec<u16>, Vec<u32>) = (Vec::new(), Vec::new());
    for kind in REGISTERED {
        if !types.contains(&kind.type_id()) {
            types.push(kind.type_id());
        }
        if !groups.contains(&kind.type_group()) {
            groups.push(kind.type_group());
        }
    }
    put(
        &mut doc,
        &format!("{schemas}/transactionTypes"),
        "enum",
        json!(types),
    )?;
    put(
        &mut doc,
        &format!("{schemas}/transactionTypeGroups"),
        "enum",
        json!(groups),
    )?;
    put(
        &mut doc,
        &format!("{schemas}/transactionVersions"),
        "enum",
        json!([2, 3]),
    )?;

    let days = days_text(patches.missed_blocks_lookback);
    for route in [
        "~1blocks~1missed",
        "~1delegates~1{identifier}~1blocks~1missed",
    ] {
        let get = format!("/paths/{route}/get");
        let summary = doc.pointer(&format!("{get}/summary"))?.as_str()?;
        let summary = summary.replacen('X', &days, 1);
        put(&mut doc, &get, "summary", json!(summary))?;
    }
    Some(doc)
}

/// Set `key` of the object at `parent`; an existing key keeps its place, a new one goes last.
fn put(doc: &mut Value, parent: &str, key: &str, value: Value) -> Option<()> {
    doc.pointer_mut(parent)?
        .as_object_mut()?
        .insert(key.to_owned(), value);
    Some(())
}

/// The pattern at `at` with its second character (the reference's address character) replaced by
/// `character`, as `pattern.substring(0, 1) + character + pattern.substring(2)`.
fn pattern(doc: &Value, at: &str, character: &str) -> Option<String> {
    let old = doc.pointer(at)?.as_str()?;
    Some(format!("{}{character}{}", old.get(..1)?, old.get(2..)?))
}

/// The address of the empty passphrase on the network with `pub_key_hash`, which the reference
/// takes for its example addresses and whose first character it writes into the patterns.
fn dummy_address(pub_key_hash: u8) -> Option<String> {
    let keys = KeyPair::from_passphrase("").ok()?;
    Some(Address::from_public_key(keys.public_key(), pub_key_hash).to_base58())
}

/// `+(seconds / 86400).toFixed(2)` as JavaScript writes the number: the days of the lookback
/// window, to two places.
fn days_text(seconds: i64) -> String {
    let days = seconds as f64 / 86_400.0;
    let magnitude = days.abs();
    // `toFixed` takes the larger candidate on an exact tie, where Rust rounds to even. A tie needs
    // the value's exact binary expansion to end in a half hundredth: it is an odd number of
    // eighths.
    let eighths = magnitude * 8.0;
    let fixed = if eighths.fract() == 0.0 && eighths % 2.0 == 1.0 && eighths < 9.0e15 {
        let hundredths = (25 * eighths as u128).div_ceil(2);
        format!("{}.{:02}", hundredths / 100, hundredths % 100)
    } else {
        format!("{magnitude:.2}")
    };
    let value: f64 = fixed.parse().unwrap_or(magnitude);
    js::number_to_string(if days < 0.0 { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patches(pub_key_hash: u8) -> Patches {
        Patches {
            pub_key_hash,
            max_transactions_per_request: 40,
            base_path: "/api".to_owned(),
            version: "4.3.1-heartwood.0.1.0".to_owned(),
            missed_blocks_lookback: 604_800,
        }
    }

    #[test]
    fn the_assets_parse_and_every_patched_key_is_there() {
        let document = base().expect("the document parses");
        assert!(patched(document, &patches(90)).is_some());
        assert!(transaction_schemas().is_ok());
    }

    #[test]
    fn the_patches_write_the_networks_character_and_the_options() {
        let document = base().unwrap();
        let doc = patched(document, &patches(30)).unwrap();
        let schemas = &doc["components"]["schemas"];
        let address = schemas["address"]["pattern"].as_str().unwrap();
        assert!(address.starts_with("^D("), "{address}");
        assert_eq!(
            schemas["recipientId"]["pattern"],
            schemas["address"]["pattern"]
        );
        assert!(
            schemas["walletIdentifier"]["pattern"]
                .as_str()
                .unwrap()
                .starts_with("^D(")
        );
        let dummy = schemas["recipientId"]["example"].as_str().unwrap();
        assert!(dummy.starts_with('D'));
        assert_eq!(
            schemas["transaction"]["properties"]["asset"]["example"]["transfers"][0]["recipientId"],
            dummy
        );
        assert_eq!(
            schemas["transactions"]["properties"]["transactions"]["maxItems"],
            40
        );
        assert_eq!(doc["servers"], json!([{ "url": "/api" }]));
        assert_eq!(doc["info"]["version"], "4.3.1-heartwood.0.1.0");
        assert_eq!(schemas["transactionTypes"]["enum"], json!([2, 7, 6, 1, 0]));
        assert_eq!(schemas["transactionTypeGroups"]["enum"], json!([1, 2]));
        assert_eq!(schemas["transactionVersions"]["enum"], json!([2, 3]));
        let summary = doc["paths"]["/blocks/missed"]["get"]["summary"]
            .as_str()
            .unwrap();
        assert!(summary.contains(" 7 "), "{summary}");
        let keys: Vec<&String> = schemas["recipientId"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["type", "pattern", "example"]);
    }

    #[test]
    fn the_document_lists_no_lock_route() {
        let doc = patched(base().unwrap(), &patches(90)).unwrap();
        let paths = doc["paths"].as_object().unwrap();
        assert!(paths.keys().all(|path| !path.starts_with("/locks")));
        assert!(paths.contains_key("/blocks/missed"));
    }

    #[test]
    fn the_days_are_written_as_to_fixed_writes_them() {
        assert_eq!(days_text(604_800), "7");
        assert_eq!(days_text(2_592_000), "30");
        assert_eq!(days_text(129_600), "1.5");
        assert_eq!(days_text(0), "0");
        // Ties, where toFixed takes the larger: 0.125, 0.375, 0.625 and 0.875 days.
        assert_eq!(days_text(10_800), "0.13");
        assert_eq!(days_text(32_400), "0.38");
        assert_eq!(days_text(54_000), "0.63");
        assert_eq!(days_text(75_600), "0.88");
        assert_eq!(days_text(1_000), "0.01");
        assert_eq!(days_text(100), "0");
        assert_eq!(days_text(-10_800), "-0.13");
        assert_eq!(days_text(-100), "0");
    }
}
