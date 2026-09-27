//! The crypto files: `config.load`.
//!
//! A case gives a milestone file as JSON text, which is loaded under the devnet network with the
//! merged milestones, rewards and donations as output (`subjects::blocks`), or all four files as
//! text (`files`, keyed by file name, a missing file left out), which the kernel's loader checks
//! with the strict schema. Accepted: `{accepted: true, milestones}` with the merged milestones.
//!
//! A refusal of the four files is a documented difference when the reference loads them: the
//! strict schema, the all-four rule and the value rules of the milestone and network loaders
//! (`Documented::StrictSchema`), or the fee and revoke keys' types (`Documented::FeeKeyValueType`).
//! Whether the files have the refused form is decided here on the input, with this module's own
//! statement of the schema, one predicate per refusal kind; a refusal of any other kind is one the
//! reference shares (a file that is not JSON, a block time that changes, a bad burn or donation).

use heartwood_crypto::crypto::slots;
use heartwood_crypto::errors::ConfigError;
use heartwood_crypto::utils::js;
use heartwood_kernel::config::crypto::{self, CryptoTexts, FILES};
use heartwood_kernel::config::{CryptoFilesError, KeyProblem, SchemaError};
use serde_json::{Map, Value, json};

use super::{Env, Outcome, Run, failed, unclassified};
use crate::subject::{Case, Documented, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("config.load", config_load)];

/// `config.load`.
fn config_load(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    match case.input.get("files") {
        None => super::blocks::config_load(env, case),
        Some(files) => load_files(files),
    }
}

/// The four files of `files`, loaded by the kernel's loader.
fn load_files(files: &Value) -> Outcome<Verdict> {
    let map = files
        .as_object()
        .ok_or_else(|| failed("input.files is not an object"))?;
    if let Some(name) = map.keys().find(|name| !FILES.contains(&name.as_str())) {
        return Err(failed(format!("input.files.{name} is not a crypto file")));
    }
    let text = |file: &str| match map.get(file) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(failed(format!("input.files.{file} is not text"))),
    };
    let [network, milestones, genesis, exceptions] = FILES.map(text);
    let texts = CryptoTexts {
        network: network?,
        milestones: milestones?,
        genesis: genesis?,
        exceptions: exceptions?,
    };
    Ok(match crypto::parse(&texts) {
        Ok(files) => {
            let merged: Vec<Value> = (files.milestones.all().iter())
                .map(|params| Value::Object(params.as_json().clone()))
                .collect();
            Verdict::Output(json!({ "accepted": true, "milestones": merged }))
        }
        Err(error) => match documented(&error, &Parsed::of(&texts)) {
            Some((difference, holds)) => Verdict::RefusedByDesign { difference, holds },
            None => unclassified(error),
        },
    })
}

/// The files of a case, parsed; `None` for a file that is missing or not JSON.
struct Parsed {
    network: Option<Value>,
    milestones: Option<Value>,
    exceptions: Option<Value>,
    missing: Vec<&'static str>,
}

impl Parsed {
    fn of(texts: &CryptoTexts) -> Parsed {
        let parse = |text: &Option<String>| {
            text.as_deref()
                .and_then(|text| serde_json::from_str(text).ok())
        };
        let all = [
            &texts.network,
            &texts.milestones,
            &texts.genesis,
            &texts.exceptions,
        ];
        Parsed {
            network: parse(&texts.network),
            milestones: parse(&texts.milestones),
            exceptions: parse(&texts.exceptions),
            missing: (FILES.into_iter().zip(all))
                .filter_map(|(file, text)| text.is_none().then_some(file))
                .collect(),
        }
    }

    /// The milestone entries of the file, in file order.
    fn entries(&self) -> &[Value] {
        self.milestones
            .as_ref()
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
}

/// The documented difference of Heartwood's refusal `error` of the files `parsed`, and whether the
/// files have its form; `None` for a refusal the reference shares.
fn documented(error: &CryptoFilesError, parsed: &Parsed) -> Option<(Documented, bool)> {
    let strict = |holds| Some((Documented::StrictSchema, holds));
    match error {
        CryptoFilesError::Missing { files } => {
            strict(!files.is_empty() && files.iter().all(|file| parsed.missing.contains(file)))
        }
        CryptoFilesError::Read { .. } | CryptoFilesError::Json { .. } => None,
        CryptoFilesError::Schema(SchemaError::Exceptions) => strict(
            parsed
                .exceptions
                .as_ref()
                .and_then(Value::as_object)
                .is_none_or(|map| !map.is_empty()),
        ),
        CryptoFilesError::Schema(SchemaError::DuplicateHeight { height }) => {
            let at = |entry: &&Value| entry_height(entry) == Some(*height);
            strict(parsed.entries().iter().filter(at).count() >= 2)
        }
        CryptoFilesError::Schema(SchemaError::Key {
            file,
            height,
            path,
            problem,
        }) => strict(key_form(parsed, file, *height, path, *problem)),
        CryptoFilesError::Config {
            file: "network.json",
            source,
        } => strict(network_form(parsed.network.as_ref(), source)),
        CryptoFilesError::Config { source, .. } => milestone_form(parsed, source),
    }
}

// The strict schema, stated independently of the kernel's loader.

/// The keys of each object of a milestone that has a fixed key set, by its dotted path.
const MILESTONE_OBJECTS: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "height",
            "activeDelegates",
            "blockTime",
            "epoch",
            "reward",
            "block",
            "blocksToRevokeDelegateResignation",
            "burn",
            "dynamicFees",
            "fees",
            "transfer",
            "dynamicReward",
            "donations",
            "p2p",
            "legacyTransfer",
            "legacyVote",
            "bip340",
            "acceptLegacySchnorrTransactions",
            "htlcEnabled",
        ],
    ),
    ("block", &["version", "maxTransactions", "maxPayload"]),
    ("burn", &["feeBasisPoints", "txAmount"]),
    ("dynamicFees", &["enabled", "minFee", "addonBytes"]),
    ("dynamicFees.addonBytes", TYPE_KEYS),
    ("fees", &["staticFees"]),
    ("fees.staticFees", TYPE_KEYS),
    ("transfer", &["minimum", "maximum"]),
    ("dynamicReward", &["enabled", "ranks", "secondaryReward"]),
    ("p2p", &["minimumVersions"]),
];

/// The keys of each object of `network.json`, by its dotted path.
const NETWORK_OBJECTS: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "name",
            "messagePrefix",
            "addressCharacter",
            "bip32",
            "pubKeyHash",
            "nethash",
            "wif",
            "slip44",
            "client",
        ],
    ),
    ("bip32", &["public", "private"]),
    ("client", &["token", "symbol", "explorer"]),
];

/// The type keys of the reference's transaction types, the removed ones included.
const TYPE_KEYS: &[&str] = &[
    "legacyTransfer",
    "secondSignature",
    "delegateRegistration",
    "legacyVote",
    "multiSignature",
    "ipfs",
    "transfer",
    "delegateResignation",
    "htlcLock",
    "htlcClaim",
    "htlcRefund",
    "burn",
    "vote",
];

/// The removed features' keys and their only accepted values.
const REMOVED: &[(&str, bool)] = &[
    ("legacyTransfer", false),
    ("legacyVote", false),
    ("bip340", true),
    ("acceptLegacySchnorrTransactions", false),
    ("htlcEnabled", false),
];

/// The own properties of JavaScript's `Object.prototype`.
const PROTOTYPE_NAMES: &[&str] = &[
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// A milestone's height, when it is an integer from 1 to `2^32 − 1`.
fn entry_height(entry: &Value) -> Option<u64> {
    let height = entry.get("height").and_then(js::safe_integer)?;
    u64::try_from(height)
        .ok()
        .filter(|height| (1..=u64::from(u32::MAX)).contains(height))
}

/// The value at the dotted `path` of `root`; array items are numbered.
fn at<'v>(root: &'v Value, path: &str) -> Option<&'v Value> {
    if path.is_empty() {
        return Some(root);
    }
    path.split('.').try_fold(root, |node, key| match node {
        Value::Object(map) => map.get(key),
        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => None,
    })
}

/// Whether a strict-schema refusal of `path` (in `file`, in the milestone at `height`) has its
/// form in the input.
fn key_form(
    parsed: &Parsed,
    file: &str,
    height: Option<u64>,
    path: &str,
    problem: KeyProblem,
) -> bool {
    let roots: Vec<&Value> = match file {
        "network.json" => parsed.network.iter().collect(),
        "milestones.json" => (parsed.entries().iter())
            .filter(|entry| height.is_none_or(|h| entry_height(entry) == Some(h)))
            .collect(),
        _ => Vec::new(),
    };
    let objects = if file == "network.json" {
        NETWORK_OBJECTS
    } else {
        MILESTONE_OBJECTS
    };
    let (parent, key) = path.rsplit_once('.').unwrap_or(("", path));
    roots.iter().any(|root| {
        let Some(value) = at(root, path) else {
            return false;
        };
        match problem {
            KeyProblem::Unknown => objects
                .iter()
                .any(|(object, keys)| *object == parent && !keys.contains(&key)),
            KeyProblem::Null => value.is_null(),
            KeyProblem::Type(expected) => match expected {
                "an object" => !value.is_object(),
                "an array of strings" => !value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string)),
                "an integer from 0 to 2^53 - 1" => {
                    js::safe_integer(value).is_none_or(|amount| amount < 0)
                }
                _ => false,
            },
            KeyProblem::RemovedFeature => {
                (REMOVED.iter()).any(|(name, off)| *name == path && *value != Value::Bool(*off))
            }
        }
    })
}

/// Whether a refusal of the network loader has its form: the file is not an object, or the key it
/// names is missing or of another type than its field.
fn network_form(network: Option<&Value>, error: &ConfigError) -> bool {
    let Some(network) = network else {
        return false;
    };
    match error {
        ConfigError::Shape(_) => !network.is_object(),
        ConfigError::Network { key, .. } => {
            let integer_below = |limit: i64| {
                move |value: &Value| {
                    js::safe_integer(value).is_some_and(|n| (0..limit).contains(&n))
                }
            };
            let fits: Box<dyn Fn(&Value) -> bool> = match key.as_str() {
                "bip32" | "client" => Box::new(Value::is_object),
                "pubKeyHash" | "wif" => Box::new(integer_below(256)),
                "bip32.public" | "bip32.private" | "slip44" => Box::new(integer_below(1 << 32)),
                _ => Box::new(Value::is_string),
            };
            at(network, key).is_none_or(|value| !fits(value))
        }
        _ => false,
    }
}

/// The documented difference of a refusal of the milestone loader, and whether its form holds:
/// the fee and revoke keys' types, or the strict schema's value rules; `None` where the reference
/// refuses the file too.
fn milestone_form(parsed: &Parsed, error: &ConfigError) -> Option<(Documented, bool)> {
    let entries = parsed.entries();
    let strict = |holds| Some((Documented::StrictSchema, holds));
    match error {
        // A milestone that is not an object: the reference skips a later one that is not an
        // array. A file that is not an array, or an empty one, both refuse.
        ConfigError::Shape(_) => entries
            .iter()
            .any(|entry| !entry.is_object())
            .then_some((Documented::StrictSchema, true)),
        ConfigError::Height { index } => strict(
            entries
                .get(*index)
                .is_some_and(|entry| entry_height(entry).is_none()),
        ),
        ConfigError::ReservedKey { index, path } => {
            let last = path.rsplit('.').next().unwrap_or(path);
            let named = entries.get(*index).and_then(|entry| at(entry, path));
            strict(named.is_some() && PROTOTYPE_NAMES.contains(&last))
        }
        ConfigError::Missing { height, key } | ConfigError::Invalid { height, key, .. } => {
            if key.starts_with("dynamicFees") || key == "blocksToRevokeDelegateResignation" {
                return Some((Documented::FeeKeyValueType, fee_form(entries)));
            }
            let active = entries
                .first()
                .and_then(|first| first.get("activeDelegates"));
            let active = active.and_then(js::safe_integer).unwrap_or(0);
            // The loader checks every entry of an enabled rank table, the reference the active
            // ranks only.
            let inactive_rank = key
                .strip_prefix("dynamicReward.ranks.")
                .is_some_and(|rank| {
                    !(rank.parse::<i64>()).is_ok_and(|rank| (1..=active).contains(&rank))
                });
            let checked = [
                "height",
                "epoch",
                "block",
                "block.version",
                "block.maxTransactions",
                "block.maxPayload",
                "transfer",
                "transfer.minimum",
                "transfer.maximum",
                "reward",
            ];
            (checked.contains(&key.as_str()) || inactive_rank)
                .then(|| (Documented::StrictSchema, value_form(entries, *height, key)))
        }
        _ => None,
    }
}

/// Whether the value of `key` at `height`, as the milestones state it, breaks the value rule the
/// milestone loader refused it for.
fn value_form(entries: &[Value], height: u64, key: &str) -> bool {
    if key == "height" {
        return entries.iter().filter_map(entry_height).min() != Some(1);
    }
    // The value the merge gives: the last one stated at or below `height`, in height order.
    let mut stated: Vec<(u64, &Value)> = (entries.iter())
        .filter_map(|entry| Some((entry_height(entry)?, at(entry, key)?)))
        .filter(|(at_height, _)| *at_height <= height)
        .collect();
    stated.sort_by_key(|(at_height, _)| *at_height);
    let Some((_, value)) = stated.last() else {
        // Nothing states it: a required key is missing.
        return matches!(key, "epoch" | "block" | "block.version" | "transfer");
    };
    let integer_in = |low: i64| {
        js::safe_integer(value).is_none_or(|n| !(low..=i64::from(u32::MAX)).contains(&n))
    };
    match key {
        // The format is the one the crypto crate documents for epochs, read with its parser.
        "epoch" => value.as_str().and_then(slots::epoch_unix_ms).is_none(),
        "block" | "transfer" => !value.is_object(),
        "block.version" => integer_in(0),
        "block.maxTransactions" | "block.maxPayload" => integer_in(1),
        "transfer.minimum" | "transfer.maximum" => js::is_truthy(value) && integer_in(1),
        _ => !value.is_null() && !is_reward_amount(value),
    }
}

/// A block reward: a non-negative safe integer, or a canonical decimal string that fits 64 bits.
fn is_reward_amount(value: &Value) -> bool {
    match value {
        Value::Number(_) => js::safe_integer(value).is_some_and(|n| n >= 0),
        Value::String(text) => {
            (text == "0" || (!text.starts_with('0') && !text.is_empty()))
                && text.bytes().all(|b| b.is_ascii_digit())
                && text.parse::<u64>().is_ok()
        }
        _ => false,
    }
}

/// Whether a milestone states a fee or revoke key of another type than the milestone loader
/// reads: `dynamicFees` neither an object nor `null`; `enabled` not a boolean, `minFee` or an
/// `addonBytes` value not a safe integer, `addonBytes` not an object, or an enabled table without
/// `minFee` or `addonBytes`; or a revoke delay that is not a safe integer.
fn fee_form(entries: &[Value]) -> bool {
    let not_safe = |value: &Value| js::safe_integer(value).is_none();
    let bad_table = |table: &Map<String, Value>| {
        let enabled = table.get("enabled");
        let addons = match table.get("addonBytes") {
            None => false,
            Some(Value::Object(addons)) => addons.values().any(not_safe),
            Some(_) => true,
        };
        enabled.is_some_and(|value| !value.is_boolean())
            || table.get("minFee").is_some_and(not_safe)
            || addons
            || (enabled == Some(&Value::Bool(true))
                && !(table.contains_key("minFee") && table.contains_key("addonBytes")))
    };
    entries.iter().filter_map(Value::as_object).any(|entry| {
        let table = match entry.get("dynamicFees") {
            None | Some(Value::Null) => false,
            Some(Value::Object(table)) => bad_table(table),
            Some(_) => true,
        };
        table
            || entry
                .get("blocksToRevokeDelegateResignation")
                .is_some_and(not_safe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reward_amounts() {
        for accepted in [
            json!(0),
            json!(5.0),
            json!("0"),
            json!("18446744073709551615"),
        ] {
            assert!(is_reward_amount(&accepted), "{accepted}");
        }
        for refused in [
            json!(-1),
            json!(1.5),
            json!("01"),
            json!(""),
            json!("18446744073709551616"),
            json!(true),
        ] {
            assert!(!is_reward_amount(&refused), "{refused}");
        }
    }

    #[test]
    fn paths() {
        let value = json!({"a": {"b": [1, {"c": null}]}});
        assert_eq!(at(&value, "a.b.1.c"), Some(&Value::Null));
        assert_eq!(at(&value, "a.b.0"), Some(&json!(1)));
        assert_eq!(at(&value, "a.x"), None);
        assert_eq!(at(&value, ""), Some(&value));
    }
}
