//! The canonical JSON of the status tree: the kernel's `stringify`, which passes the sorted list
//! of every key in the tree to `JSON.stringify` as its replacer. For the status tree this is JSON
//! with the keys of every object sorted by UTF-16 code units, no white space, strings escaped as
//! `JSON.stringify` escapes them, integers as decimal text, and keys whose value is `undefined`
//! left out.
//!
//! One more rule comes from how `stringify` collects the keys: it skips every key that is `in` an
//! empty object, so a key named like a property of `Object.prototype` (`constructor`, `toString`,
//! ...) never reaches the replacer list and is left out wherever it appears, a plugin name included.
//!
//! The signer and the verifier write the same tree from the same messages ([`preimage`]); they
//! differ only in the slot they pass and in whether an empty `plugins` is written.

use std::fmt::Write as _;

use crate::codecs::Amount;
use crate::js;
use crate::messages::peer::{Config, Plugin, State, StatusHeader};

/// The names of the properties of `Object.prototype` in Node 18, which `stringify` leaves out.
const PROTOTYPE_NAMES: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

/// A value of the status tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    /// An object's own keys; a key whose value is `undefined` is not listed.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// An object of the entries whose value is not `undefined` (`None`).
    fn object<const N: usize>(entries: [(&str, Option<Json>); N]) -> Json {
        Json::Object(
            entries
                .into_iter()
                .filter_map(|(key, value)| Some((key.to_owned(), value?)))
                .collect(),
        )
    }

    fn text(text: &Option<String>) -> Option<Json> {
        text.clone().map(Json::Str)
    }

    fn int(value: Option<u32>) -> Option<Json> {
        value.map(|n| Json::Int(i64::from(n)))
    }
}

/// Append the canonical JSON of `value` to `out`. The status tree has a fixed depth, so the
/// recursion is bounded whatever the input.
pub(super) fn write(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Json::Int(value) => {
            let _ = write!(out, "{value}");
        }
        Json::Str(text) => js::json_string(out, text),
        Json::Object(entries) => {
            let mut kept: Vec<&(String, Json)> = entries
                .iter()
                .filter(|(key, _)| !PROTOTYPE_NAMES.contains(&key.as_str()))
                .collect();
            kept.sort_by(|a, b| js::utf16_cmp(&a.0, &b.0));
            out.push('{');
            for (index, (key, value)) in kept.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                js::json_string(out, key);
                out.push(':');
                write(value, out);
            }
            out.push('}');
        }
    }
}

/// The preimage of a status: the canonical JSON of `{state, config}` as UTF-8.
///
/// `state` gives the present fields of the state and its header, `current_slot` the slot to
/// write, and `amounts` the header's total amount, total fee and reward, which are always
/// written. `config` is written as its own fields (`toJSON`); an empty `plugins` is written only
/// when `empty_plugins` is true, as the signer's plain object does. A plugin without a value is
/// `null`.
pub(super) fn preimage(
    state: &State,
    current_slot: Option<i64>,
    amounts: [Amount; 3],
    config: &Config,
    empty_plugins: bool,
) -> Vec<u8> {
    let header = state.header.as_ref().map(|h| header(h, amounts));
    let state = Json::object([
        ("height", Json::int(state.height)),
        ("forgingAllowed", state.forging_allowed.map(Json::Bool)),
        ("currentSlot", current_slot.map(Json::Int)),
        ("header", header),
    ]);
    let network = config.network.as_ref().map(|network| {
        let token = network.token.as_ref().map(|token| {
            Json::object([
                ("name", Json::text(&token.name)),
                ("symbol", Json::text(&token.symbol)),
            ])
        });
        Json::object([
            ("name", Json::text(&network.name)),
            ("nethash", Json::text(&network.nethash)),
            ("explorer", Json::text(&network.explorer)),
            ("token", token),
            ("version", Json::int(network.version)),
        ])
    });
    let plugins = (empty_plugins || !config.plugins.is_empty()).then(|| {
        Json::Object(
            config
                .plugins
                .iter()
                .map(|(name, plugin)| (name.to_owned(), plugin.map_or(Json::Null, plugin_json)))
                .collect(),
        )
    });
    let config = Json::object([
        ("version", Json::text(&config.version)),
        ("network", network),
        ("plugins", plugins),
    ]);
    let mut out = String::new();
    write(
        &Json::object([("state", Some(state)), ("config", Some(config))]),
        &mut out,
    );
    out.into_bytes()
}

fn header(header: &StatusHeader, [total_amount, total_fee, reward]: [Amount; 3]) -> Json {
    let amount = |value: Amount| Some(Json::Str(value.to_string()));
    Json::object([
        ("id", Json::text(&header.id)),
        ("version", Json::int(header.version)),
        ("timestamp", Json::int(header.timestamp)),
        ("previousBlock", Json::text(&header.previous_block)),
        ("height", Json::int(header.height)),
        (
            "numberOfTransactions",
            Json::int(header.number_of_transactions),
        ),
        ("totalAmount", amount(total_amount)),
        ("totalFee", amount(total_fee)),
        ("reward", amount(reward)),
        ("payloadLength", Json::int(header.payload_length)),
        ("payloadHash", Json::text(&header.payload_hash)),
        (
            "generatorPublicKey",
            Json::text(&header.generator_public_key),
        ),
        ("blockSignature", Json::text(&header.block_signature)),
    ])
}

fn plugin_json(plugin: &Plugin) -> Json {
    Json::object([
        ("port", Json::int(plugin.port)),
        ("enabled", plugin.enabled.map(Json::Bool)),
        (
            "estimateTotalCount",
            plugin.estimate_total_count.map(Json::Bool),
        ),
    ])
}
