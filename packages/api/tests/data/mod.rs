//! What the search tests read: the records of the vector class V34 and the data files beside this
//! one (made by `generate.js`).
//!
//! The class is read from `tests/vectors`, or from the run that `API_VECTORS` names (a path from
//! the workspace root, such as `target/vectors/local` after `make vectors-local`). The crate's own
//! directory must hold the class, so a missing class fails the test. A run named in the
//! environment may lack it, and the test then says so and passes. A kind below its minimum in
//! `tools/oracle/vector-minimums.json` fails the test either way.

#![allow(dead_code, reason = "each test file uses a part of the data")]

use std::path::Path;

use serde_json::{Value, json};

const CLASS: &str = "V34-api-tiers";

/// The records of `op` in class V34, or `None` when the run `API_VECTORS` names lacks the class.
pub fn vector_records(op: &str) -> Option<Vec<Value>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = std::env::var_os("API_VECTORS").filter(|dir| !dir.is_empty());
    let dir = named.as_ref().map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(_) if named.is_some() => {
            eprintln!("{CLASS} is not in the directory API_VECTORS names");
            return None;
        }
        Err(error) => panic!("{}: {error} (make vectors-install)", file.display()),
    };
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    let found: Vec<Value> = lines.filter(|record| record["op"] == op).collect();
    let minimums: Value = serde_json::from_str(
        &std::fs::read_to_string(manifest.join("../../tools/oracle/vector-minimums.json")).unwrap(),
    )
    .unwrap();
    let minimum = minimums[CLASS][op].as_u64().unwrap();
    assert!(
        found.len() as u64 >= minimum,
        "{op}: {} records, minimum {minimum}",
        found.len()
    );
    Some(found)
}
