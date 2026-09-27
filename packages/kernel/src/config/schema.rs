//! The strict schema of the crypto files.
//!
//! `milestones.json`, beyond the crypto crate's checks: no key outside the known ones at any
//! level (the file shape and key names stay the reference's), no `null` at any depth, no height
//! twice, and the keys of removed features only at their off values (`legacyTransfer`,
//! `legacyVote`, `acceptLegacySchnorrTransactions` and `htlcEnabled` false, `bip340` true). The
//! free-form maps keep their own rules: the keys of `dynamicFees.addonBytes` and
//! `fees.staticFees` are transaction type keys (of every type the reference registers) with
//! integer values from 0 to `2^53 − 1`, and `dynamicReward.ranks` and `donations` are the crypto
//! crate's to check. `p2p.minimumVersions` is an array of strings, and every key that groups
//! others holds an object. `network.json`: exactly the reference's keys, no `null`.
//! `exceptions.json`: exactly `{}`. A refusal names the file, the height where there is one, and
//! the key path. The value of every other key is checked by the crypto crate's loaders.

use heartwood_crypto::utils::js;
use serde_json::Value;
use thiserror::Error;

/// A crypto file breaks the strict schema.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SchemaError {
    /// A key, or a value inside it, breaks the schema.
    #[error("{file}{}: {path} {problem}", height.map(|h| format!(" at height {h}")).unwrap_or_default())]
    Key {
        /// The file.
        file: &'static str,
        /// The milestone's height, for a milestone key whose milestone has a valid height.
        height: Option<u64>,
        /// The key's dotted path; array items are numbered from 0.
        path: String,
        /// What is wrong with it.
        problem: KeyProblem,
    },
    /// Two milestones at the same height.
    #[error("milestones.json: two milestones at height {height}")]
    DuplicateHeight {
        /// The height.
        height: u64,
    },
    /// `exceptions.json` is not exactly `{}`.
    #[error("exceptions.json must be an empty object")]
    Exceptions,
}

/// What is wrong with a key of a crypto file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum KeyProblem {
    /// The file's schema does not know the key.
    #[error("is not a known key")]
    Unknown,
    /// The value is `null`.
    #[error("is null")]
    Null,
    /// The value is of another type than the key's; the text says what the key takes.
    #[error("must be {0}")]
    Type(&'static str),
    /// A removed feature's key at another value than its off value.
    #[error("turns on a removed feature")]
    RemovedFeature,
}

/// What a key holds.
#[derive(Debug, Clone, Copy)]
enum Rule {
    /// Anything without a `null`; the crypto crate's loaders check the value.
    Any,
    /// An object with these keys.
    Object(&'static [(&'static str, Rule)]),
    /// An integer from 0 to `2^53 − 1`.
    Amount,
    /// An array of strings.
    Strings,
    /// A removed feature, which may only have this value.
    Off(bool),
    /// A key the schema does not know.
    Unknown,
}

use Rule::{Amount, Any, Object, Off, Strings, Unknown};

/// The keys of a milestone.
#[rustfmt::skip]
const MILESTONE: &[(&str, Rule)] = &[
    ("height", Any), ("activeDelegates", Any), ("blockTime", Any), ("epoch", Any), ("reward", Any),
    ("block", Object(&[("version", Any), ("maxTransactions", Any), ("maxPayload", Any)])),
    ("blocksToRevokeDelegateResignation", Any),
    ("burn", Object(&[("feeBasisPoints", Any), ("txAmount", Any)])),
    ("dynamicFees", Object(&[("enabled", Any), ("minFee", Any), ("addonBytes", Object(FEES))])),
    ("fees", Object(&[("staticFees", Object(FEES))])),
    ("transfer", Object(&[("minimum", Any), ("maximum", Any)])),
    ("dynamicReward", Object(&[("enabled", Any), ("ranks", Any), ("secondaryReward", Any)])),
    ("donations", Any), ("p2p", Object(&[("minimumVersions", Strings)])),
    ("legacyTransfer", Off(false)), ("legacyVote", Off(false)), ("bip340", Off(true)),
    ("acceptLegacySchnorrTransactions", Off(false)), ("htlcEnabled", Off(false)),
];

/// The keys of `network.json`.
#[rustfmt::skip]
const NETWORK: &[(&str, Rule)] = &[
    ("name", Any), ("messagePrefix", Any), ("addressCharacter", Any), ("pubKeyHash", Any),
    ("bip32", Object(&[("public", Any), ("private", Any)])), ("nethash", Any), ("wif", Any),
    ("slip44", Any), ("client", Object(&[("token", Any), ("symbol", Any), ("explorer", Any)])),
];

/// A fee table: the type keys of every transaction type the reference registers, removed ones
/// included, each with an integer from 0 to `2^53 − 1`.
#[rustfmt::skip]
const FEES: &[(&str, Rule)] = &[
    ("legacyTransfer", Amount), ("secondSignature", Amount), ("delegateRegistration", Amount),
    ("legacyVote", Amount), ("multiSignature", Amount), ("ipfs", Amount), ("transfer", Amount),
    ("delegateResignation", Amount), ("htlcLock", Amount), ("htlcClaim", Amount),
    ("htlcRefund", Amount), ("burn", Amount), ("vote", Amount),
];

/// Where a value is: the file and the milestone's height.
struct Place {
    file: &'static str,
    height: Option<u64>,
}

impl Place {
    /// Check `value`, at the dotted `path`, against `rule`.
    fn check(&self, path: &str, value: &Value, rule: Rule) -> Result<(), SchemaError> {
        let fail = |problem| {
            Err(SchemaError::Key {
                file: self.file,
                height: self.height,
                path: path.to_owned(),
                problem,
            })
        };
        let inner = |key: &str| format!("{path}{}{key}", if path.is_empty() { "" } else { "." });
        match (rule, value) {
            (Unknown, _) => fail(KeyProblem::Unknown),
            (_, Value::Null) => fail(KeyProblem::Null),
            (Object(keys), Value::Object(map)) => map.iter().try_for_each(|(key, item)| {
                let rule = keys.iter().find(|(known, _)| known == key);
                self.check(&inner(key), item, rule.map_or(Unknown, |(_, rule)| *rule))
            }),
            (Any, Value::Object(map)) => {
                (map.iter()).try_for_each(|(key, item)| self.check(&inner(key), item, Any))
            }
            (Any, Value::Array(items)) => (items.iter().enumerate())
                .try_for_each(|(index, item)| self.check(&inner(&index.to_string()), item, Any)),
            (Off(off), _) if *value != Value::Bool(off) => fail(KeyProblem::RemovedFeature),
            (Amount, _) if js::safe_integer(value).is_some_and(|amount| amount >= 0) => Ok(()),
            (Strings, Value::Array(items)) if items.iter().all(Value::is_string) => Ok(()),
            (Any | Off(_), _) => Ok(()),
            (Object(_), _) => fail(KeyProblem::Type("an object")),
            (Amount, _) => fail(KeyProblem::Type("an integer from 0 to 2^53 - 1")),
            (Strings, _) => fail(KeyProblem::Type("an array of strings")),
        }
    }
}

/// Check the parsed `milestones.json`. A file that is not an array of objects, or a milestone
/// without a usable height, is left to the crypto crate's loader, which refuses it.
pub fn check_milestones(json: &Value) -> Result<(), SchemaError> {
    let (mut heights, file) = (Vec::new(), "milestones.json");
    for milestone in json.as_array().into_iter().flatten() {
        let height = milestone.get("height").and_then(js::safe_integer);
        let height = height.and_then(|height| u64::try_from(height).ok());
        if let Some(height) = height.filter(|height| heights.contains(height)) {
            return Err(SchemaError::DuplicateHeight { height });
        }
        heights.extend(height);
        if milestone.is_object() {
            Place { file, height }.check("", milestone, Object(MILESTONE))?;
        }
    }
    Ok(())
}

/// Check the parsed `network.json`. A file that is not an object is left to the crypto crate's
/// loader, which refuses it.
pub fn check_network(json: &Value) -> Result<(), SchemaError> {
    let (file, height) = ("network.json", None);
    json.as_object().map_or(Ok(()), |_| {
        Place { file, height }.check("", json, Object(NETWORK))
    })
}

/// Check the parsed `exceptions.json`: exactly `{}`.
pub fn check_exceptions(json: &Value) -> Result<(), SchemaError> {
    match json {
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(SchemaError::Exceptions),
    }
}
