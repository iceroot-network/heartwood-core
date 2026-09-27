//! The cases of the crypto files: the devnet's milestone and network files mutated (keys added at
//! random paths, `null` values, a height twice, value types changed, removed features turned on,
//! files left out), for `config.load`.

use std::sync::Arc;

use heartwood_crypto::utils::js;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use serde_json::{Map, Value, json};

use crate::strategies::kernel::OpCase;
use crate::strategies::transactions::TxContext;

/// The file names, as `config.load` takes them.
const FILES: [&str; 4] = [
    "network.json",
    "milestones.json",
    "genesisBlock.json",
    "exceptions.json",
];

/// Keys that mutations add: known keys, keys of removed features and of the reference's older
/// rule sets, and names no schema knows.
const KEYS: &[&str] = &[
    "aip11",
    "vendorFieldLength",
    "multiPaymentLimit",
    "x",
    "height",
    "reward",
    "epoch",
    "transfer",
    "minimum",
    "version",
    "enabled",
    "staticFees",
    "vote",
    "ipfs",
    "feePercent",
    "legacyTransfer",
    "legacyVote",
    "bip340",
    "acceptLegacySchnorrTransactions",
    "htlcEnabled",
    "minimumVersions",
    "explorer",
];

/// The removed features' keys.
const REMOVED: [&str; 5] = [
    "legacyTransfer",
    "legacyVote",
    "bip340",
    "acceptLegacySchnorrTransactions",
    "htlcEnabled",
];

/// A change to the devnet's files.
#[derive(Debug, Clone)]
enum Mutation {
    /// Add `key` with `value` to the object the choices lead to in the milestone or network file.
    AddKey {
        network: bool,
        choices: Vec<usize>,
        key: &'static str,
        value: Value,
    },
    /// Set the value the choices lead to to `null`.
    Null { network: bool, choices: Vec<usize> },
    /// Change the type of the value the choices lead to.
    ChangeType {
        network: bool,
        choices: Vec<usize>,
        how: usize,
    },
    /// Give milestone `to` the height of milestone `from`, or append a copy of `from`.
    DuplicateHeight { from: usize, to: usize, copy: bool },
    /// Set a removed feature's key of milestone `at` to `value`.
    Removed {
        at: usize,
        key: &'static str,
        value: Value,
    },
    /// Leave out the files whose bits are set.
    Remove(u8),
    /// Replace `exceptions.json`.
    Exceptions(Value),
}

/// Small JSON values of every type.
fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-5i64..=300_000_000).prop_map(|n| json!(n)),
        Just(json!(1.5)),
        "[a-z0-9]{0,6}".prop_map(Value::String),
        Just(json!([])),
        Just(json!([">=4.3.0"])),
        Just(json!({})),
        Just(json!({"a": 1})),
    ]
}

fn choices() -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(any::<usize>(), 1..=4)
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        4 => (prop::bool::weighted(0.2), choices(), prop::sample::select(KEYS), values())
            .prop_map(|(network, choices, key, value)| Mutation::AddKey { network, choices, key, value }),
        3 => (prop::bool::weighted(0.2), choices())
            .prop_map(|(network, choices)| Mutation::Null { network, choices }),
        4 => (prop::bool::weighted(0.2), choices(), 0usize..4)
            .prop_map(|(network, choices, how)| Mutation::ChangeType { network, choices, how }),
        1 => (0usize..4, 0usize..4, any::<bool>())
            .prop_map(|(from, to, copy)| Mutation::DuplicateHeight { from, to, copy }),
        2 => (0usize..4, prop::sample::select(REMOVED.to_vec()), prop_oneof![any::<bool>().prop_map(Value::Bool), values()])
            .prop_map(|(at, key, value)| Mutation::Removed { at, key, value }),
        1 => (1u8..16).prop_map(Mutation::Remove),
        1 => prop_oneof![Just(json!({"transactions": []})), Just(json!([])), Just(Value::Null), Just(json!({"blocks": ["ab"]}))]
            .prop_map(Mutation::Exceptions),
    ]
}

/// The JSON pointer that `choices` lead to in `root`: at each object or array the child
/// `choice % length`, stopping at a leaf; the root itself for no choice.
fn pointer(root: &Value, choices: &[usize], objects_only: bool) -> String {
    let mut node = root;
    let mut path = String::new();
    for choice in choices {
        let next = match node {
            Value::Object(map) if !map.is_empty() => map
                .iter()
                .nth(choice % map.len())
                .map(|(key, value)| (key.clone(), value)),
            Value::Array(items) if !items.is_empty() => {
                let index = choice % items.len();
                items.get(index).map(|value| (index.to_string(), value))
            }
            _ => None,
        };
        let Some((key, value)) = next else { break };
        if objects_only && !value.is_object() {
            break;
        }
        path.push('/');
        path.push_str(&key.replace('~', "~0").replace('/', "~1"));
        node = value;
    }
    path
}

/// `value` with another type: a number as text or a boolean, text as a number, an object as an
/// array and back, `how` choosing among them.
fn retyped(value: &Value, how: usize) -> Value {
    match value {
        Value::Number(number) if how.is_multiple_of(2) => Value::String(number.to_string()),
        Value::Number(number) => Value::Bool(js::number_value(number) != 0.0),
        Value::String(text) if how.is_multiple_of(2) => json!(text.len()),
        Value::String(_) => json!([]),
        Value::Bool(flag) if how.is_multiple_of(2) => json!(u8::from(*flag)),
        Value::Bool(flag) => Value::String(flag.to_string()),
        Value::Object(map) => Value::Array(map.values().take(how).cloned().collect()),
        Value::Array(_) if how.is_multiple_of(2) => Value::Object(Map::new()),
        Value::Array(items) => json!(items.len()),
        Value::Null => json!(0),
    }
}

/// Apply `mutation` to the parsed network and milestone files, the exceptions file and the set
/// of files present.
fn apply(
    mutation: &Mutation,
    network: &mut Value,
    milestones: &mut Value,
    exceptions: &mut Value,
    present: &mut [bool; 4],
) {
    match mutation {
        Mutation::AddKey {
            network: in_network,
            choices,
            key,
            value,
        } => {
            let target = if *in_network {
                &mut *network
            } else {
                &mut *milestones
            };
            let at = pointer(target, choices, true);
            if let Some(Value::Object(map)) = target.pointer_mut(&at) {
                map.insert((*key).to_owned(), value.clone());
            }
        }
        Mutation::Null {
            network: in_network,
            choices,
        } => {
            let target = if *in_network {
                &mut *network
            } else {
                &mut *milestones
            };
            let at = pointer(target, choices, false);
            if let Some(value) = target.pointer_mut(&at) {
                *value = Value::Null;
            }
        }
        Mutation::ChangeType {
            network: in_network,
            choices,
            how,
        } => {
            let target = if *in_network {
                &mut *network
            } else {
                &mut *milestones
            };
            let at = pointer(target, choices, false);
            if let Some(value) = target.pointer_mut(&at) {
                *value = retyped(value, *how);
            }
        }
        Mutation::DuplicateHeight { from, to, copy } => {
            if let Some(entries) = milestones.as_array_mut()
                && !entries.is_empty()
            {
                let length = entries.len();
                let source = entries.get(from % length).cloned().unwrap_or(Value::Null);
                if *copy {
                    entries.push(source);
                } else if let Some(Value::Object(target)) = entries.get_mut(to % length) {
                    target.insert("height".into(), source["height"].clone());
                }
            }
        }
        Mutation::Removed { at, key, value } => {
            if let Some(entries) = milestones.as_array_mut()
                && !entries.is_empty()
            {
                let length = entries.len();
                if let Some(Value::Object(entry)) = entries.get_mut(at % length) {
                    entry.insert((*key).to_owned(), value.clone());
                }
            }
        }
        Mutation::Remove(bits) => {
            for (index, present) in present.iter_mut().enumerate() {
                if bits & (1 << index) != 0 {
                    *present = false;
                }
            }
        }
        Mutation::Exceptions(value) => *exceptions = value.clone(),
    }
}

/// The cases of `config.load` over the devnet files of `context`: the files unchanged in one case
/// in eight, otherwise with one or two mutations.
pub fn config_cases(context: Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let mutations = prop_oneof![
        1 => Just(Vec::new()),
        7 => prop::collection::vec(mutation(), 1..=2),
    ];
    mutations
        .prop_map(move |mutations| {
            let mut network = context.network.to_json();
            let mut milestones: Value =
                serde_json::from_str(&context.milestones_text).unwrap_or(Value::Null);
            let mut exceptions = json!({});
            let mut present = [true; 4];
            for mutation in &mutations {
                apply(
                    mutation,
                    &mut network,
                    &mut milestones,
                    &mut exceptions,
                    &mut present,
                );
            }
            let texts = [
                js::stringify(&network, 4) + "\n",
                js::stringify(&milestones, 4) + "\n",
                "{\n    \"height\": 1,\n    \"transactions\": []\n}\n".to_owned(),
                js::stringify(&exceptions, 4) + "\n",
            ];
            let files: Map<String, Value> = (FILES.iter().zip(texts).zip(present))
                .filter(|(_, present)| *present)
                .map(|((file, text), _)| ((*file).to_owned(), Value::String(text)))
                .collect();
            ("config.load", 2, json!({ "files": files }))
        })
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_follow_the_choices() {
        let value = json!({"a": {"b": [1, {"c": 2}]}, "d": 3});
        assert_eq!(pointer(&value, &[0, 0, 1, 0], false), "/a/b/1/c");
        assert_eq!(pointer(&value, &[1, 5], false), "/d");
        assert_eq!(pointer(&value, &[0, 0], true), "/a");
        assert_eq!(pointer(&value, &[], false), "");
    }

    #[test]
    fn types_change() {
        for value in [
            json!(1),
            json!("a"),
            json!(true),
            json!({"a": 1}),
            json!([1]),
            Value::Null,
        ] {
            for how in 0..4 {
                let changed = retyped(&value, how);
                assert_ne!(
                    std::mem::discriminant(&changed),
                    std::mem::discriminant(&value),
                    "{value} {how}"
                );
            }
        }
    }
}
