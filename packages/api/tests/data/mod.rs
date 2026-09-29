//! What the search tests read: the records of the vector class V34 and the data files beside this
//! one (made by `generate.js`).

#![allow(dead_code, reason = "each test file uses a part of the data")]

use std::path::Path;

use serde_json::Value;

/// The records of `op` in class V34, or `None` when the class is not at hand.
pub fn vector_records(op: &str) -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("API_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let Ok(text) = std::fs::read_to_string(dir.join("V34-api-tiers.jsonl")) else {
        eprintln!(
            "class V34 is not in {}: its records are not compared",
            dir.display()
        );
        return None;
    };
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    Some(records.filter(|record| record["op"] == op).collect())
}
