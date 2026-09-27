//! The strict schema of the crypto files.
//!
//! `milestones.json`, beyond the crypto crate's checks: no key outside the known ones at any
//! level (the file shape and key names stay the reference's), no `null` at any depth, no height
//! twice, and the keys of removed features only at their off values. The free-form maps
//! (`dynamicFees.addonBytes`, `fees.staticFees`, `dynamicReward.ranks`, `donations`) keep their
//! own rules. `network.json`: exactly the reference's keys, no `null`. `exceptions.json`: exactly
//! `{}`. A refusal names the file, the height where there is one, and the key path.

use serde_json::Value;
use thiserror::Error;

/// A crypto file breaks the strict schema.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaError {
    /// A key that the file's schema does not know.
    #[error("{file}{}: unknown key {path}", at(*height))]
    UnknownKey {
        /// The file.
        file: &'static str,
        /// The milestone's height, for a milestone key.
        height: Option<u64>,
        /// The key's dotted path.
        path: String,
    },
    /// A `null` value.
    #[error("{file}{}: {path} is null", at(*height))]
    Null {
        /// The file.
        file: &'static str,
        /// The milestone's height, for a milestone key.
        height: Option<u64>,
        /// The key's dotted path.
        path: String,
    },
    /// A value of another type than the key's.
    #[error("{file}{}: {path} must be {expected}", at(*height))]
    Type {
        /// The file.
        file: &'static str,
        /// The milestone's height, for a milestone key.
        height: Option<u64>,
        /// The key's dotted path.
        path: String,
        /// What the key takes.
        expected: &'static str,
    },
    /// Two milestones at the same height.
    #[error("milestones.json: two milestones at height {height}")]
    DuplicateHeight {
        /// The height.
        height: u64,
    },
    /// A removed feature's key at another value than its off value.
    #[error("milestones.json at height {height}: {key} turns on a removed feature")]
    RemovedFeature {
        /// The milestone's height.
        height: u64,
        /// The key.
        key: String,
    },
    /// `exceptions.json` is not exactly `{}`.
    #[error("exceptions.json must be an empty object")]
    Exceptions,
}

/// ` at height H` for a milestone key, or nothing.
fn at(height: Option<u64>) -> String {
    height.map_or_else(String::new, |height| format!(" at height {height}"))
}

/// Check the parsed `milestones.json`.
pub fn check_milestones(json: &Value) -> Result<(), SchemaError> {
    let _ = json;
    todo!("the milestone schema")
}

/// Check the parsed `network.json`.
pub fn check_network(json: &Value) -> Result<(), SchemaError> {
    let _ = json;
    todo!("the network schema")
}

/// Check the parsed `exceptions.json`.
pub fn check_exceptions(json: &Value) -> Result<(), SchemaError> {
    let _ = json;
    todo!("exceptions.json is empty")
}
