//! The protobuf messages in the oracle's JSON, and `proto.encode` and `proto.decode`: one message
//! without a route's post-processing, named as the reference names it (`shared.Headers`,
//! `peer.GetStatusResponse.Config`).
//!
//! A message's JSON has its present keys only, bytes as hex, and the map as an object whose `null`
//! values are entries without a value. A `uint32` takes any integer and keeps its low 32 bits, as
//! the reference's writer does (a negative slot). A decoded message's JSON has the reference's own
//! keys in their order ([`Proto::own_keys`]). [`Json`] serves the area subjects too.

use heartwood_crypto::utils::hex;
use heartwood_p2p_wire::messages::blocks::{
    BlockHeader, GetBlocksRequest, GetBlocksResponse, PostBlockRequest, PostBlockResponse,
};
use heartwood_p2p_wire::messages::peer::{
    Common, Config, GetCommonBlocksRequest, GetCommonBlocksResponse, GetPeersRequest,
    GetPeersResponse, GetStatusRequest, GetStatusResponse, Network, Peer, Plugin, Plugins, State,
    StatusHeader, Token,
};
use heartwood_p2p_wire::messages::shared::Headers;
use heartwood_p2p_wire::messages::transactions::{
    GetUnconfirmedTransactionsRequest, GetUnconfirmedTransactionsResponse, PostTransactionsRequest,
    PostTransactionsResponse,
};
use heartwood_p2p_wire::{DecodeError, Proto};
use serde_json::{Map, Value, json};

use super::{accepted, bytes_value, object, refused};
use crate::subject::{Case, Documented, Verdict};
use crate::subjects::{Env, Outcome, failed, hex_bytes, rejected, text};

/// A message in the oracle's JSON.
pub(super) trait Json: Proto {
    /// The message of `value`: its present keys only.
    fn from_json(value: &Value) -> Outcome<Self>;

    /// The message's JSON: its own keys, in the reference's order.
    fn to_json(&self) -> Outcome<Value>;
}

/// A field in the oracle's JSON.
trait JsonField: Sized {
    fn from_json(value: &Value) -> Outcome<Self>;
    fn to_json(&self) -> Outcome<Value>;
}

/// The JSON of an absent field, which is never an own key.
fn absent() -> Outcome<Value> {
    Err(failed("an absent field has no JSON"))
}

impl JsonField for Option<u32> {
    fn from_json(value: &Value) -> Outcome<Self> {
        match value {
            Value::Null => Ok(None),
            // ToUint32 of an integer: its low 32 bits.
            _ => value
                .as_i64()
                .map(|n| n as u32)
                .or_else(|| value.as_u64().map(|n| n as u32))
                .map(Some)
                .ok_or_else(|| failed(format!("{value} is not an integer"))),
        }
    }

    fn to_json(&self) -> Outcome<Value> {
        self.map_or_else(absent, |n| Ok(json!(n)))
    }
}

impl JsonField for Option<bool> {
    fn from_json(value: &Value) -> Outcome<Self> {
        match value {
            Value::Null => Ok(None),
            Value::Bool(b) => Ok(Some(*b)),
            _ => Err(failed(format!("{value} is not a boolean"))),
        }
    }

    fn to_json(&self) -> Outcome<Value> {
        self.map_or_else(absent, |b| Ok(Value::Bool(b)))
    }
}

impl JsonField for Option<String> {
    fn from_json(value: &Value) -> Outcome<Self> {
        match value {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(failed(format!("{value} is not text"))),
        }
    }

    fn to_json(&self) -> Outcome<Value> {
        self.as_ref()
            .map_or_else(absent, |text| Ok(Value::String(text.clone())))
    }
}

impl JsonField for Option<Vec<u8>> {
    fn from_json(value: &Value) -> Outcome<Self> {
        match value {
            Value::Null => Ok(None),
            Value::String(text) => hex::decode(text)
                .map(Some)
                .map_err(|e| failed(format!("{text}: {e}"))),
            _ => Err(failed(format!("{value} is not hex"))),
        }
    }

    fn to_json(&self) -> Outcome<Value> {
        self.as_ref()
            .map_or_else(absent, |bytes| Ok(bytes_value(bytes)))
    }
}

impl<M: Json> JsonField for Option<M> {
    fn from_json(value: &Value) -> Outcome<Self> {
        match value {
            Value::Null => Ok(None),
            _ => M::from_json(value).map(Some),
        }
    }

    fn to_json(&self) -> Outcome<Value> {
        self.as_ref().map_or_else(absent, Json::to_json)
    }
}

impl JsonField for Vec<String> {
    fn from_json(value: &Value) -> Outcome<Self> {
        value
            .as_array()
            .ok_or_else(|| failed(format!("{value} is not an array")))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| failed(format!("{item} is not text")))
            })
            .collect()
    }

    fn to_json(&self) -> Outcome<Value> {
        Ok(json!(self))
    }
}

impl<M: Json> JsonField for Vec<M> {
    fn from_json(value: &Value) -> Outcome<Self> {
        list_from_json(value)
    }

    fn to_json(&self) -> Outcome<Value> {
        list_json(self)
    }
}

impl JsonField for Plugins {
    fn from_json(value: &Value) -> Outcome<Self> {
        let entries = value
            .as_object()
            .ok_or_else(|| failed(format!("{value} is not an object")))?;
        let mut plugins = Plugins::new();
        for (name, plugin) in entries {
            plugins.insert(name.clone(), JsonField::from_json(plugin)?);
        }
        Ok(plugins)
    }

    fn to_json(&self) -> Outcome<Value> {
        let mut entries = Map::new();
        for (name, plugin) in self.iter() {
            let value = plugin.map_or(Ok(Value::Null), Json::to_json)?;
            entries.insert(name.to_owned(), value);
        }
        Ok(Value::Object(entries))
    }
}

/// Heartwood's verdict on a decode: the JSON that `json` gives of the value, or the refusal.
/// Groups nested beyond the reference's stack are the documented difference where the oracle's
/// answer shows it, whatever Heartwood's verdict.
pub(super) fn verdict<T>(
    result: Result<T, DecodeError>,
    json: impl FnOnce(&T) -> Outcome<Value>,
) -> Outcome<Verdict> {
    Ok(match result {
        Ok(value) => accepted(json(&value)?),
        Err(error) => Verdict::DiffersWhereShown {
            difference: Documented::DeepGroups,
            verdict: Box::new(refused(error)),
        },
    })
}

/// The JSON of a list of messages.
pub(super) fn list_json<M: Json>(messages: &[M]) -> Outcome<Value> {
    messages
        .iter()
        .map(Json::to_json)
        .collect::<Outcome<Vec<Value>>>()
        .map(Value::Array)
}

/// The messages of a JSON array.
pub(super) fn list_from_json<M: Json>(value: &Value) -> Outcome<Vec<M>> {
    value
        .as_array()
        .ok_or_else(|| failed(format!("{value} is not an array")))?
        .iter()
        .map(M::from_json)
        .collect()
}

/// `proto.encode`: `{hex}` of `input.value` as the message `input.message`.
pub(super) fn encode(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let bytes = encode_named(text(case.input, "message")?, &case.input["value"])?;
    Ok(Verdict::Output(json!({ "hex": hex::encode(&bytes) })))
}

/// `proto.decode`: `{value}` of `input.hex` as the message `input.message`.
pub(super) fn decode(_env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let name = text(case.input, "message")?;
    let Some(bytes) = hex_bytes(case.input, "hex")? else {
        return Ok(rejected("TypeError"));
    };
    decode_named(name, &bytes)
        .map(|verdict| super::map_output(verdict, &|value| json!({ "value": value })))
}

/// The [`Json`] implementations of messages, and the functions that encode and decode a message
/// by its name.
macro_rules! json_messages {
    ($($name:literal $message:ident { $($field:ident $js:literal),* $(,)? })*) => {
        $(
            impl Json for $message {
                fn from_json(value: &Value) -> Outcome<Self> {
                    let object = value
                        .as_object()
                        .ok_or_else(|| failed(format!("{}: {value} is not an object", $name)))?;
                    if let Some(key) = object.keys().find(|key| ![$($js),*].contains(&key.as_str())) {
                        return Err(failed(format!("{}: no field {key}", $name)));
                    }
                    let mut message = $message::default();
                    $(
                        if let Some(value) = object.get($js) {
                            message.$field = JsonField::from_json(value)?;
                        }
                    )*
                    Ok(message)
                }

                fn to_json(&self) -> Outcome<Value> {
                    object(self, |key| match key {
                        $($js => JsonField::to_json(&self.$field),)*
                        _ => Err(failed(format!("{}: no field {key}", $name))),
                    })
                }
            }
        )*

        /// The bytes of `value` as the message named `name`.
        fn encode_named(name: &str, value: &Value) -> Outcome<Vec<u8>> {
            match name {
                $($name => Ok($message::from_json(value)?.encode()),)*
                _ => Err(failed(format!("no message named {name}"))),
            }
        }

        /// The verdict on `bytes` decoded as the message named `name`; its output is the value.
        fn decode_named(name: &str, bytes: &[u8]) -> Outcome<Verdict> {
            match name {
                $($name => verdict($message::decode(bytes), Json::to_json),)*
                _ => Err(failed(format!("no message named {name}"))),
            }
        }
    };
}

json_messages! {
    "shared.Headers" Headers { version "version", port "port" }
    "peer.GetPeersRequest" GetPeersRequest { headers "headers" }
    "peer.GetPeersResponse" GetPeersResponse { peers "peers" }
    "peer.GetPeersResponse.Peer" Peer { ip "ip", port "port" }
    "peer.GetCommonBlocksRequest" GetCommonBlocksRequest { ids "ids", headers "headers" }
    "peer.GetCommonBlocksResponse" GetCommonBlocksResponse { common "common" }
    "peer.GetCommonBlocksResponse.Common" Common { height "height", id "id" }
    "peer.GetStatusRequest" GetStatusRequest { headers "headers" }
    "peer.GetStatusResponse" GetStatusResponse {
        state "state", config "config", public_keys "publicKeys", signatures "signatures"
    }
    "peer.GetStatusResponse.State" State {
        height "height", forging_allowed "forgingAllowed", current_slot "currentSlot",
        header "header"
    }
    "peer.GetStatusResponse.State.BlockHeader" StatusHeader {
        id "id", version "version", timestamp "timestamp", previous_block "previousBlock",
        height "height", number_of_transactions "numberOfTransactions",
        total_amount "totalAmount", total_fee "totalFee", reward "reward",
        payload_length "payloadLength", payload_hash "payloadHash",
        generator_public_key "generatorPublicKey", block_signature "blockSignature"
    }
    "peer.GetStatusResponse.Config" Config { version "version", network "network", plugins "plugins" }
    "peer.GetStatusResponse.Config.Network" Network {
        name "name", nethash "nethash", explorer "explorer", token "token", version "version"
    }
    "peer.GetStatusResponse.Config.Network.Token" Token { name "name", symbol "symbol" }
    "peer.GetStatusResponse.Config.Plugin" Plugin {
        port "port", enabled "enabled", estimate_total_count "estimateTotalCount"
    }
    "blocks.GetBlocksRequest" GetBlocksRequest {
        last_block_height "lastBlockHeight", block_limit "blockLimit",
        headers_only "headersOnly", serialised "serialised", headers "headers"
    }
    "blocks.GetBlocksResponse" GetBlocksResponse { blocks "blocks" }
    "blocks.GetBlocksResponse.BlockHeader" BlockHeader {
        id "id", version "version", timestamp "timestamp", previous_block "previousBlock",
        height "height", number_of_transactions "numberOfTransactions",
        total_amount "totalAmount", total_fee "totalFee", reward "reward",
        payload_length "payloadLength", payload_hash "payloadHash",
        generator_public_key "generatorPublicKey", block_signature "blockSignature",
        transactions "transactions"
    }
    "blocks.PostBlockRequest" PostBlockRequest { block "block", headers "headers" }
    "blocks.PostBlockResponse" PostBlockResponse { status "status", height "height" }
    "transactions.GetUnconfirmedTransactionsRequest" GetUnconfirmedTransactionsRequest {
        count_only "countOnly", exclude "exclude", headers "headers"
    }
    "transactions.GetUnconfirmedTransactionsResponse" GetUnconfirmedTransactionsResponse {
        pool_size "poolSize", transactions "transactions", headers "headers"
    }
    "transactions.PostTransactionsRequest" PostTransactionsRequest {
        transactions "transactions", headers "headers"
    }
    "transactions.PostTransactionsResponse" PostTransactionsResponse { accept "accept" }
}
