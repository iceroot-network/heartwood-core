//! The signed status: the getStatus response codec, `status.sign` (the relay's tree, preimage and
//! attestations over a block) and `status.verify` (decode, reply check, both preimages and the
//! signature checks of a received response).
//!
//! Every verdict on a status payload may be the documented difference of protobuf groups nested
//! beyond the reference's stack, which the oracle's answer shows; a reply whose plugins set the
//! reference object's prototype is the documented difference of that name.

use heartwood_crypto::blocks::deserialiser::deserialise_header;
use heartwood_crypto::utils::bytes::Reader;
use heartwood_crypto::utils::hex;
use heartwood_crypto::{KeyPair, SecretKey};
use heartwood_p2p_wire::messages::peer::{
    Config, GetStatusResponse, Network, Plugin, Plugins, State, StatusHeader, Token,
};
use heartwood_p2p_wire::status::{
    self, NetworkConfig, PeerConfig, PluginConfig, ReceivedStatus, SignedState, StatusHeaderData,
    TokenConfig,
};
use heartwood_p2p_wire::{Amount, Proto, ReplyRefused};
use serde_json::{Map, Value, json};

use super::{accepted, object, refused};
use crate::subject::{Case, Documented, SubjectError, Verdict};
use crate::subjects::{Env, Outcome, failed, hex_bytes, rejected, unclassified};

/// The getStatus response payload of `value`: the message of its present keys, the header's
/// amounts written as the decimal text of their values.
pub(super) fn encode_response(value: &Value) -> Outcome<Vec<u8>> {
    let header = &value["state"]["header"];
    let amount = |key: &str| {
        let text = header[key]
            .as_str()
            .ok_or_else(|| failed(format!("value.state.header.{key} is not text")))?;
        Amount::parse(text).map_err(|e| failed(format!("value.state.header.{key}: {e}")))
    };
    let received = ReceivedStatus {
        message: response_message(value)?,
        total_amount: amount("totalAmount")?,
        total_fee: amount("totalFee")?,
        reward: amount("reward")?,
    };
    Ok(status::encode_response(&received))
}

/// The verdict on the getStatus response `bytes`; its output is the decoded value.
pub(super) fn decode_response(bytes: &[u8]) -> Outcome<Verdict> {
    Ok(match status::decode_response(bytes) {
        Ok(received) => accepted(response_value(&received)?),
        Err(error) => deep(refused(error)),
    })
}

/// `status.sign`: `{preimage, hex}` for a block, a slot, a configuration and keys.
pub(super) fn sign(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let input = case.input;
    let Some(block) = hex_bytes(input, "block")? else {
        return Ok(rejected("TypeError"));
    };
    let header = match deserialise_header(&mut Reader::new(&block)) {
        Ok(header) => header,
        Err(error) => return Ok(unclassified(error)),
    };
    let state = SignedState {
        height: header.height,
        forging_allowed: boolean(&input["forgingAllowed"], "input.forgingAllowed")?,
        current_slot: input["currentSlot"]
            .as_i64()
            .ok_or_else(|| failed("input.currentSlot is not an integer"))?,
        header: StatusHeaderData::from_header(&header),
    };
    let config = peer_config(&input["config"])?;
    let keys = input["keys"]
        .as_array()
        .ok_or_else(|| failed("input.keys is not a list"))?
        .iter()
        .map(|key| {
            key.as_str()
                .and_then(|key| SecretKey::from_hex(key).ok())
                .map(KeyPair::from_secret_key)
                .ok_or_else(|| failed("input.keys holds a value that is not a secret key"))
        })
        .collect::<Outcome<Vec<_>>>()?;
    let preimage = status::signing_preimage(&state, &config);
    let attestations = match keys
        .iter()
        .map(|key| status::attest(&preimage, key))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(attestations) => attestations,
        Err(error) => return Ok(unclassified(error)),
    };
    let bytes = status::response(&state, &config, &attestations).encode();
    Ok(Verdict::Output(json!({
        "preimage": hex::encode(&preimage),
        "hex": hex::encode(&bytes),
    })))
}

/// `status.verify`: `{valid, refused}` for a reply the check refuses, and `{valid, value,
/// preimage, verified, preimageNegativeSlot, verifiedNegativeSlot}` for one it accepts.
pub(super) fn verify(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    let max_delegates = match case.input.get("maxDelegates") {
        Some(value) => value.as_u64(),
        None => env
            .devnet()?
            .milestones
            .all()
            .iter()
            .map(|params| params.active_delegates())
            .max(),
    }
    .and_then(|n| usize::try_from(n).ok())
    .ok_or_else(|| failed("input.maxDelegates is not a count"))?;
    let mut received = match status::decode_response(&bytes) {
        Ok(received) => received,
        Err(error) => return Ok(deep(refused(error))),
    };
    let prototype = received
        .message
        .config
        .as_ref()
        .is_some_and(|config| config.plugins.replaced_prototype());
    let output = match status::check_reply(&mut received, max_delegates) {
        Err(part) => json!({ "valid": false, "refused": part_name(part) }),
        Ok(()) => {
            let message = &received.message;
            let pairs = |preimage: &[u8]| -> Vec<bool> {
                (message.signatures.iter().enumerate())
                    .map(|(i, signature)| {
                        (message.public_keys.get(i))
                            .is_some_and(|key| status::verify_attestation(preimage, key, signature))
                    })
                    .collect()
            };
            let preimage = status::verifying_preimage(&received, false);
            let negative = status::verifying_preimage(&received, true);
            json!({
                "valid": true,
                "value": response_value(&received)?,
                "preimage": hex::encode(&preimage),
                "verified": pairs(&preimage),
                "preimageNegativeSlot": hex::encode(&negative),
                "verifiedNegativeSlot": pairs(&negative),
            })
        }
    };
    let verdict = Verdict::Output(output);
    Ok(deep(if prototype {
        Verdict::DiffersWhereShown {
            difference: Documented::PluginsPrototype,
            verdict: Box::new(verdict),
        }
    } else {
        verdict
    }))
}

/// `verdict`, or the documented difference of deep groups where the oracle's answer shows it.
fn deep(verdict: Verdict) -> Verdict {
    Verdict::DiffersWhereShown {
        difference: Documented::DeepGroups,
        verdict: Box::new(verdict),
    }
}

/// The name of the reply schema's part that the oracle reports.
fn part_name(part: ReplyRefused) -> &'static str {
    match part {
        ReplyRefused::Root => "root",
        ReplyRefused::State => "state",
        ReplyRefused::Header => "header",
        ReplyRefused::Config => "config",
        ReplyRefused::Network => "network",
        ReplyRefused::Plugins => "plugins",
        ReplyRefused::PublicKeys => "publicKeys",
        ReplyRefused::Signatures => "signatures",
    }
}

// Heartwood's values in the oracle's shape: own keys in the reference's order, amounts as the
// decimal text of their values.

fn present<'a, T>(field: &'a Option<T>, key: &str) -> Outcome<&'a T> {
    field
        .as_ref()
        .ok_or_else(|| failed(format!("{key} is an own key without a value")))
}

fn unknown(key: &str) -> SubjectError {
    failed(format!("no field {key}"))
}

fn text(field: &Option<String>, key: &str) -> Outcome<Value> {
    present(field, key).map(|text| json!(text))
}

fn number(field: &Option<u32>, key: &str) -> Outcome<Value> {
    present(field, key).map(|n| json!(n))
}

fn flag(field: &Option<bool>, key: &str) -> Outcome<Value> {
    present(field, key).map(|b| json!(b))
}

/// The decoded response: `{...decoded, state: {...state, header: {...header, amounts}}}`.
fn response_value(received: &ReceivedStatus) -> Outcome<Value> {
    let message = &received.message;
    object(message, |key| match key {
        "state" => state_value(present(&message.state, key)?, received),
        "config" => config_value(present(&message.config, key)?),
        "publicKeys" => Ok(json!(message.public_keys)),
        "signatures" => Ok(json!(message.signatures)),
        other => Err(unknown(other)),
    })
}

fn state_value(state: &State, received: &ReceivedStatus) -> Outcome<Value> {
    object(state, |key| match key {
        "height" => number(&state.height, key),
        "forgingAllowed" => flag(&state.forging_allowed, key),
        "currentSlot" => number(&state.current_slot, key),
        "header" => header_value(present(&state.header, key)?, received),
        other => Err(unknown(other)),
    })
}

fn header_value(header: &StatusHeader, received: &ReceivedStatus) -> Outcome<Value> {
    object(header, |key| match key {
        "id" => text(&header.id, key),
        "version" => number(&header.version, key),
        "timestamp" => number(&header.timestamp, key),
        "previousBlock" => text(&header.previous_block, key),
        "height" => number(&header.height, key),
        "numberOfTransactions" => number(&header.number_of_transactions, key),
        "totalAmount" => Ok(json!(received.total_amount.to_string())),
        "totalFee" => Ok(json!(received.total_fee.to_string())),
        "reward" => Ok(json!(received.reward.to_string())),
        "payloadLength" => number(&header.payload_length, key),
        "payloadHash" => text(&header.payload_hash, key),
        "generatorPublicKey" => text(&header.generator_public_key, key),
        "blockSignature" => text(&header.block_signature, key),
        other => Err(unknown(other)),
    })
}

fn config_value(config: &Config) -> Outcome<Value> {
    object(config, |key| match key {
        "version" => text(&config.version, key),
        "network" => network_value(present(&config.network, key)?),
        "plugins" => {
            let mut plugins = Map::new();
            for (name, plugin) in config.plugins.iter() {
                let value = plugin.map_or(Ok(Value::Null), plugin_value)?;
                plugins.insert(name.to_owned(), value);
            }
            Ok(Value::Object(plugins))
        }
        other => Err(unknown(other)),
    })
}

fn network_value(network: &Network) -> Outcome<Value> {
    object(network, |key| match key {
        "name" => text(&network.name, key),
        "nethash" => text(&network.nethash, key),
        "explorer" => text(&network.explorer, key),
        "token" => {
            let token = present(&network.token, key)?;
            object(token, |key| match key {
                "name" => text(&token.name, key),
                "symbol" => text(&token.symbol, key),
                other => Err(unknown(other)),
            })
        }
        "version" => number(&network.version, key),
        other => Err(unknown(other)),
    })
}

fn plugin_value(plugin: &Plugin) -> Outcome<Value> {
    object(plugin, |key| match key {
        "port" => number(&plugin.port, key),
        "enabled" => flag(&plugin.enabled, key),
        "estimateTotalCount" => flag(&plugin.estimate_total_count, key),
        other => Err(unknown(other)),
    })
}

// The oracle's JSON as Heartwood's types: present keys only.

fn boolean(value: &Value, path: &str) -> Outcome<bool> {
    value
        .as_bool()
        .ok_or_else(|| failed(format!("{path} is not a boolean")))
}

fn string(value: &Value, path: &str) -> Outcome<String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| failed(format!("{path} is not text")))
}

fn integer<T: TryFrom<u64>>(value: &Value, path: &str) -> Outcome<T> {
    value
        .as_u64()
        .and_then(|n| T::try_from(n).ok())
        .ok_or_else(|| failed(format!("{path} is not an integer in range")))
}

/// `object[key]` read with `read`; an absent key and `null` are an absent field, as the reference's
/// encoder writes only values that are not `null` or `undefined`.
fn optional<T>(
    object: &Value,
    key: &str,
    read: impl Fn(&Value, &str) -> Outcome<T>,
) -> Outcome<Option<T>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(value, key).map(Some),
    }
}

/// A nested message: absent, or read with `read`.
fn nested<T>(object: &Value, key: &str, read: impl Fn(&Value) -> Outcome<T>) -> Outcome<Option<T>> {
    optional(object, key, |value, _| read(value))
}

fn list(object: &Value, key: &str) -> Outcome<Vec<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(|item| string(item, key)).collect(),
        Some(_) => Err(failed(format!("{key} is not a list"))),
    }
}

fn response_message(value: &Value) -> Outcome<GetStatusResponse> {
    Ok(GetStatusResponse {
        state: nested(value, "state", state_message)?,
        config: nested(value, "config", config_message)?,
        public_keys: list(value, "publicKeys")?,
        signatures: list(value, "signatures")?,
        ..GetStatusResponse::default()
    })
}

fn state_message(value: &Value) -> Outcome<State> {
    Ok(State {
        height: optional(value, "height", integer)?,
        forging_allowed: optional(value, "forgingAllowed", boolean)?,
        current_slot: optional(value, "currentSlot", integer)?,
        header: nested(value, "header", header_message)?,
        ..State::default()
    })
}

fn header_message(value: &Value) -> Outcome<StatusHeader> {
    Ok(StatusHeader {
        id: optional(value, "id", string)?,
        version: optional(value, "version", integer)?,
        timestamp: optional(value, "timestamp", integer)?,
        previous_block: optional(value, "previousBlock", string)?,
        height: optional(value, "height", integer)?,
        number_of_transactions: optional(value, "numberOfTransactions", integer)?,
        total_amount: optional(value, "totalAmount", string)?,
        total_fee: optional(value, "totalFee", string)?,
        reward: optional(value, "reward", string)?,
        payload_length: optional(value, "payloadLength", integer)?,
        payload_hash: optional(value, "payloadHash", string)?,
        generator_public_key: optional(value, "generatorPublicKey", string)?,
        block_signature: optional(value, "blockSignature", string)?,
        ..StatusHeader::default()
    })
}

fn config_message(value: &Value) -> Outcome<Config> {
    let mut plugins = Plugins::new();
    if let Some(entries) = value.get("plugins") {
        let entries = entries
            .as_object()
            .ok_or_else(|| failed("plugins is not an object"))?;
        for (name, plugin) in entries {
            let plugin = match plugin {
                Value::Null => None,
                plugin => Some(plugin_message(plugin)?),
            };
            plugins.insert(name.clone(), plugin);
        }
    }
    Ok(Config {
        version: optional(value, "version", string)?,
        network: nested(value, "network", network_message)?,
        plugins,
        ..Config::default()
    })
}

fn network_message(value: &Value) -> Outcome<Network> {
    Ok(Network {
        name: optional(value, "name", string)?,
        nethash: optional(value, "nethash", string)?,
        explorer: optional(value, "explorer", string)?,
        token: nested(value, "token", |token| {
            Ok(Token {
                name: optional(token, "name", string)?,
                symbol: optional(token, "symbol", string)?,
                ..Token::default()
            })
        })?,
        version: optional(value, "version", integer)?,
        ..Network::default()
    })
}

fn plugin_message(value: &Value) -> Outcome<Plugin> {
    Ok(Plugin {
        port: optional(value, "port", integer)?,
        enabled: optional(value, "enabled", boolean)?,
        estimate_total_count: optional(value, "estimateTotalCount", boolean)?,
        ..Plugin::default()
    })
}

/// A node's configuration in `status.sign`'s input: the plugins as `[name, plugin]` pairs, in the
/// configuration's order.
fn peer_config(value: &Value) -> Outcome<PeerConfig> {
    let network = &value["network"];
    let plugins = value["plugins"]
        .as_array()
        .ok_or_else(|| failed("input.config.plugins is not a list"))?
        .iter()
        .map(|entry| {
            let name = string(&entry[0], "a plugin name")?;
            let plugin = &entry[1];
            Ok((
                name,
                PluginConfig {
                    enabled: boolean(&plugin["enabled"], "a plugin's enabled")?,
                    port: integer(&plugin["port"], "a plugin's port")?,
                    estimate_total_count: optional(plugin, "estimateTotalCount", boolean)?,
                },
            ))
        })
        .collect::<Outcome<Vec<_>>>()?;
    Ok(PeerConfig {
        version: string(&value["version"], "input.config.version")?,
        network: NetworkConfig {
            version: integer(&network["version"], "input.config.network.version")?,
            name: string(&network["name"], "input.config.network.name")?,
            nethash: string(&network["nethash"], "input.config.network.nethash")?,
            explorer: string(&network["explorer"], "input.config.network.explorer")?,
            token: TokenConfig {
                name: string(&network["token"]["name"], "input.config.network.token.name")?,
                symbol: string(
                    &network["token"]["symbol"],
                    "input.config.network.token.symbol",
                )?,
            },
        },
        plugins,
    })
}
