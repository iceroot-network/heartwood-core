//! The canonical JSON of the status tree: the kernel's `stringify`, which passes the sorted list
//! of every key in the tree to `JSON.stringify` as its replacer. For the status tree this is JSON
//! with the keys of every object sorted by UTF-16 code units, no white space, strings escaped as
//! `JSON.stringify` escapes them, integers as decimal text, and keys whose value is `undefined`
//! left out.
//!
//! One more rule comes from how `stringify` collects the keys: it skips every key that is `in` an
//! empty object, so a key named like a property of `Object.prototype` (`constructor`, `toString`,
//! ...) never reaches the replacer list and is left out wherever it appears, a plugin name included.
//! The replacer list still reaches inherited properties: see [`inherited`].
//!
//! The signer and the verifier write the same tree from the same messages ([`preimage`]); they
//! differ only in the slot they pass and in whether an empty `plugins` is written.

use std::fmt::Write as _;

use crate::codecs::Amount;
use crate::js;
use crate::messages::peer::{Config, Plugin, Plugins, State, StatusHeader};

/// The names of the properties of `Object.prototype` in Node 18, which `stringify` leaves out.
const PROTOTYPE_NAMES: &str = "constructor __defineGetter__ __defineSetter__ hasOwnProperty \
    __lookupGetter__ __lookupSetter__ isPrototypeOf propertyIsEnumerable toString valueOf __proto__ \
    toLocaleString";

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

/// An object of the entries whose value is not `undefined` (`None`).
fn object<const N: usize>(entries: [(&str, Option<Json>); N]) -> Json {
    let present = entries
        .into_iter()
        .filter_map(|(key, value)| Some((key.to_owned(), value?)));
    Json::Object(present.collect())
}

fn text(text: &Option<String>) -> Option<Json> {
    text.clone().map(Json::Str)
}

fn int(value: Option<u32>) -> Option<Json> {
    value.map(|n| Json::Int(i64::from(n)))
}

fn flag(value: Option<bool>) -> Option<Json> {
    value.map(Json::Bool)
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
            let listed = |key: &str| !PROTOTYPE_NAMES.split_whitespace().any(|name| name == key);
            let mut kept: Vec<_> = entries.iter().filter(|(key, _)| listed(key)).collect();
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
    let state = object([
        ("height", int(state.height)),
        ("forgingAllowed", flag(state.forging_allowed)),
        ("currentSlot", current_slot.map(Json::Int)),
        ("header", state.header.as_ref().map(|h| header(h, amounts))),
    ]);
    let network = config.network.as_ref().map(|network| {
        let token = (network.token.as_ref())
            .map(|token| object([("name", text(&token.name)), ("symbol", text(&token.symbol))]));
        object([
            ("name", text(&network.name)),
            ("nethash", text(&network.nethash)),
            ("explorer", text(&network.explorer)),
            ("token", token),
            ("version", int(network.version)),
        ])
    });
    let plugins = (empty_plugins || !config.plugins.is_empty()).then(|| {
        let entries = config.plugins.iter();
        let own = entries.map(|(name, p)| (name.to_owned(), p.map_or(Json::Null, plugin)));
        Json::Object(own.chain(inherited(&config.plugins)).collect())
    });
    let config = object([
        ("version", text(&config.version)),
        ("network", network),
        ("plugins", plugins),
    ]);
    let mut out = String::new();
    write(
        &object([("state", Some(state)), ("config", Some(config))]),
        &mut out,
    );
    out.into_bytes()
}

fn header(header: &StatusHeader, [total_amount, total_fee, reward]: [Amount; 3]) -> Json {
    let amount = |value: Amount| Some(Json::Str(value.to_string()));
    object([
        ("id", text(&header.id)),
        ("version", int(header.version)),
        ("timestamp", int(header.timestamp)),
        ("previousBlock", text(&header.previous_block)),
        ("height", int(header.height)),
        ("numberOfTransactions", int(header.number_of_transactions)),
        ("totalAmount", amount(total_amount)),
        ("totalFee", amount(total_fee)),
        ("reward", amount(reward)),
        ("payloadLength", int(header.payload_length)),
        ("payloadHash", text(&header.payload_hash)),
        ("generatorPublicKey", text(&header.generator_public_key)),
        ("blockSignature", text(&header.block_signature)),
    ])
}

/// The fields that the reference's plugins object inherits. `Config.toObject` copies the own
/// entries onto a new object, so an own entry named `__proto__` (which only a `null` prototype
/// allows) becomes the new object's prototype. The replacer list then reaches each field of that
/// plugin that some other entry has, unless an entry of that name shadows it.
fn inherited(plugins: &Plugins) -> Vec<(String, Json)> {
    type Read = fn(&Plugin) -> Option<Json>;
    let prototype = plugins
        .has_own_proto_key()
        .then(|| plugins.get(js::PROTO_KEY));
    let Some(Some(Some(prototype))) = prototype else {
        return Vec::new();
    };
    let fields: [(&str, Read); 3] = [
        ("port", |p| int(p.port)),
        ("enabled", |p| flag(p.enabled)),
        ("estimateTotalCount", |p| flag(p.estimate_total_count)),
    ];
    let listed = |read: Read| {
        (plugins.iter()).any(|(name, p)| name != js::PROTO_KEY && p.and_then(read).is_some())
    };
    (fields.into_iter())
        .filter(|&(field, read)| plugins.get(field).is_none() && listed(read))
        .filter_map(|(field, read)| Some((field.to_owned(), read(prototype)?)))
        .collect()
}

fn plugin(plugin: &Plugin) -> Json {
    object([
        ("port", int(plugin.port)),
        ("enabled", flag(plugin.enabled)),
        ("estimateTotalCount", flag(plugin.estimate_total_count)),
    ])
}
