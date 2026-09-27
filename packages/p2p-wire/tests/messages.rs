//! The message records of vector class V13, both ways: `proto.encode` and `proto.decode` of each
//! message, and `wire.encode`, `wire.decode` and `wire.decodeSelected` of the codecs that pass a
//! message through as it is (getPeers, getCommonBlocks, the getStatus request, the getBlocks
//! request, the postBlock response and the postTransactions response).
//!
//! The records are read as `common::records` says.
//!
//! Encode records must give the oracle's bytes; decode records the oracle's value, with its keys
//! in the same order, or a refusal with the same JavaScript error class. Groups nested beyond the
//! reference's stack are a documented difference: the reference overflows, the crate decodes.

mod common;

use common::CLASS;
use heartwood_p2p_wire::codecs::{blocks, peer, transactions};
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
use heartwood_p2p_wire::{Codec, DecodeError, Proto};
use serde_json::{Map, Value, json};

/// The message of Node's `RangeError` when its stack overflows.
const STACK_OVERFLOW: &str = "Maximum call stack size exceeded";

/// A message in the oracle's JSON.
trait Json: Proto {
    fn from_json(value: &Value) -> Self;
    fn to_json(&self) -> Value;
}

/// A field in the oracle's JSON.
trait JsonField: Sized {
    fn from_json(value: &Value) -> Self;
    fn to_json(&self) -> Value;
}

impl JsonField for Option<u32> {
    fn from_json(value: &Value) -> Self {
        // ToUint32 of an integer, as the reference's writer takes it.
        (!value.is_null()).then(|| value.as_i64().expect("an integer") as u32)
    }
    fn to_json(&self) -> Value {
        json!(self.expect("present"))
    }
}

impl JsonField for Option<bool> {
    fn from_json(value: &Value) -> Self {
        value.as_bool()
    }
    fn to_json(&self) -> Value {
        json!(self.expect("present"))
    }
}

impl JsonField for Option<String> {
    fn from_json(value: &Value) -> Self {
        value.as_str().map(str::to_owned)
    }
    fn to_json(&self) -> Value {
        json!(self.as_ref().expect("present"))
    }
}

impl JsonField for Option<Vec<u8>> {
    fn from_json(value: &Value) -> Self {
        value.as_str().map(|text| hex::decode(text).unwrap())
    }
    fn to_json(&self) -> Value {
        json!(hex::encode(self.as_ref().expect("present")))
    }
}

impl<M: Json> JsonField for Option<M> {
    fn from_json(value: &Value) -> Self {
        (!value.is_null()).then(|| M::from_json(value))
    }
    fn to_json(&self) -> Value {
        self.as_ref().expect("present").to_json()
    }
}

impl JsonField for Vec<String> {
    fn from_json(value: &Value) -> Self {
        serde_json::from_value(value.clone()).unwrap()
    }
    fn to_json(&self) -> Value {
        json!(self)
    }
}

impl<M: Json> JsonField for Vec<M> {
    fn from_json(value: &Value) -> Self {
        value.as_array().unwrap().iter().map(M::from_json).collect()
    }
    fn to_json(&self) -> Value {
        Value::Array(self.iter().map(Json::to_json).collect())
    }
}

impl JsonField for Plugins {
    fn from_json(value: &Value) -> Self {
        let mut plugins = Plugins::new();
        for (name, plugin) in value.as_object().unwrap() {
            plugins.insert(name.clone(), JsonField::from_json(plugin));
        }
        plugins
    }
    fn to_json(&self) -> Value {
        let entries = self
            .iter()
            .map(|(name, plugin)| (name.to_owned(), plugin.map_or(Value::Null, Json::to_json)));
        Value::Object(entries.collect())
    }
}

macro_rules! json_messages {
    ($($name:literal $message:ident { $($field:ident $js:literal),* $(,)? })*) => {
        $(
            impl Json for $message {
                fn from_json(value: &Value) -> Self {
                    let object = value.as_object().unwrap();
                    for key in object.keys() {
                        assert!([$($js),*].contains(&key.as_str()), "{}: no field {key}", $name);
                    }
                    let mut message = $message::default();
                    $(
                        if let Some(value) = object.get($js) {
                            message.$field = JsonField::from_json(value);
                        }
                    )*
                    message
                }

                fn to_json(&self) -> Value {
                    let mut object = Map::new();
                    for key in self.own_keys() {
                        let value = match key {
                            $($js => JsonField::to_json(&self.$field),)*
                            _ => unreachable!("{}: no field {key}", $name),
                        };
                        object.insert(key.to_owned(), value);
                    }
                    Value::Object(object)
                }
            }
        )*

        fn encode_named(name: &str, value: &Value) -> Vec<u8> {
            match name {
                $($name => $message::from_json(value).encode(),)*
                _ => panic!("no message named {name}"),
            }
        }

        fn decode_named(name: &str, bytes: &[u8]) -> Result<Value, DecodeError> {
            match name {
                $($name => $message::decode(bytes).map(|message| message.to_json()),)*
                _ => panic!("no message named {name}"),
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

/// The codec of a record's `route` and `direction`, if it passes a message through.
fn plain_codec(input: &Value) -> Option<Codec> {
    let codec = Codec::ALL.into_iter().find(|codec| {
        codec.route().name() == input["route"] && codec.direction().name() == input["direction"]
    })?;
    PLAIN.contains(&codec).then_some(codec)
}

/// The codecs that pass a message through as it is.
const PLAIN: [Codec; 8] = [
    Codec::GetPeersRequest,
    Codec::GetPeersResponse,
    Codec::GetCommonBlocksRequest,
    Codec::GetCommonBlocksResponse,
    Codec::GetStatusRequest,
    Codec::GetBlocksRequest,
    Codec::PostBlockResponse,
    Codec::PostTransactionsResponse,
];

fn list<M: Json>(value: &Value) -> Vec<M> {
    value.as_array().unwrap().iter().map(M::from_json).collect()
}

fn list_json<M: Json>(messages: &[M]) -> Value {
    Value::Array(messages.iter().map(Json::to_json).collect())
}

fn encode_plain(codec: Codec, value: &Value) -> Vec<u8> {
    match codec {
        Codec::GetPeersRequest => peer::get_peers_encode_request(&Json::from_json(value)),
        Codec::GetPeersResponse => peer::get_peers_encode_response(&list(value)),
        Codec::GetCommonBlocksRequest => {
            peer::get_common_blocks_encode_request(&Json::from_json(value))
        }
        Codec::GetCommonBlocksResponse => {
            peer::get_common_blocks_encode_response(&Json::from_json(value))
        }
        Codec::GetStatusRequest => peer::get_status_encode_request(&Json::from_json(value)),
        Codec::GetBlocksRequest => blocks::get_blocks_encode_request(&Json::from_json(value)),
        Codec::PostBlockResponse => blocks::post_block_encode_response(&Json::from_json(value)),
        Codec::PostTransactionsResponse => transactions::post_transactions_encode_response(
            &serde_json::from_value::<Vec<String>>(value.clone()).unwrap(),
        ),
        other => unreachable!("{other:?}"),
    }
}

fn decode_plain(codec: Codec, bytes: &[u8]) -> Result<Value, DecodeError> {
    match codec {
        Codec::GetPeersRequest => peer::get_peers_decode_request(bytes).map(|m| m.to_json()),
        Codec::GetPeersResponse => peer::get_peers_decode_response(bytes).map(|m| list_json(&m)),
        Codec::GetCommonBlocksRequest => {
            peer::get_common_blocks_decode_request(bytes).map(|m| m.to_json())
        }
        Codec::GetCommonBlocksResponse => {
            peer::get_common_blocks_decode_response(bytes).map(|m| m.to_json())
        }
        Codec::GetStatusRequest => peer::get_status_decode_request(bytes).map(|m| m.to_json()),
        Codec::GetBlocksRequest => blocks::get_blocks_decode_request(bytes).map(|m| m.to_json()),
        Codec::PostBlockResponse => blocks::post_block_decode_response(bytes).map(|m| m.to_json()),
        Codec::PostTransactionsResponse => {
            transactions::post_transactions_decode_response(bytes).map(|m| json!(m))
        }
        other => unreachable!("{other:?}"),
    }
}

fn bytes(record: &Value) -> Vec<u8> {
    hex::decode(record["input"]["hex"].as_str().unwrap()).unwrap()
}

/// Compare a decode with the record: the same value with the same key order (compared as JSON
/// text, since a JSON map compares without order), or the same error class.
fn check_decode(
    record: &Value,
    label: &str,
    result: Result<Value, DecodeError>,
    wrap: impl Fn(Value) -> Value,
) {
    match (record.get("error"), result) {
        (None, Ok(value)) => {
            let mut expected = record["output"].as_object().unwrap().clone();
            expected.shift_remove("notes");
            assert_eq!(
                serde_json::to_string(&wrap(value)).unwrap(),
                serde_json::to_string(&expected).unwrap(),
                "{label}"
            );
        }
        (Some(error), Ok(_)) => {
            // Documented: the reference's stack overflows on deep groups, the crate decodes them.
            let message = error["message"].as_str().unwrap();
            assert!(
                error["class"] == "RangeError" && message.starts_with(STACK_OVERFLOW),
                "{label}: the reference refused ({error}), the crate accepted"
            );
        }
        (Some(error), Err(refusal)) => {
            assert_eq!(refusal.js_class(), error["class"].as_str(), "{label}");
        }
        (None, Err(refusal)) => {
            panic!("{label}: the reference accepted, the crate refused ({refusal:?})")
        }
    }
}

#[test]
fn message_records_both_ways() {
    let records = common::records();
    let mut checked = 0;
    for (index, record) in records.iter().enumerate() {
        let label = format!("{CLASS}:{} {} {}", index + 2, record["op"], record["name"]);
        let input = &record["input"];
        match record["op"].as_str().unwrap() {
            "proto.encode" => {
                let hex = hex::encode(encode_named(
                    input["message"].as_str().unwrap(),
                    &input["value"],
                ));
                assert_eq!(hex, record["output"]["hex"], "{label}");
            }
            "proto.decode" => {
                let result = decode_named(input["message"].as_str().unwrap(), &bytes(record));
                check_decode(record, &label, result, |value| json!({ "value": value }));
            }
            "wire.encode" => {
                let Some(codec) = plain_codec(input) else {
                    continue;
                };
                let hex = hex::encode(encode_plain(codec, &input["value"]));
                assert_eq!(hex, record["output"]["hex"], "{label}");
            }
            "wire.decode" => {
                let Some(codec) = plain_codec(input) else {
                    continue;
                };
                check_decode(
                    record,
                    &label,
                    decode_plain(codec, &bytes(record)),
                    |value| json!({ "value": value }),
                );
            }
            "wire.decodeSelected" => {
                let all = bytes(record);
                let (first, payload) = all.split_first().map_or((0, &[][..]), |(b, p)| (*b, p));
                let codec = Codec::select(first);
                if !PLAIN.contains(&codec) {
                    continue;
                }
                let wrap = |value| json!({ "route": codec.route().name(), "direction": codec.direction().name(), "value": value });
                check_decode(record, &label, decode_plain(codec, payload), wrap);
            }
            _ => continue,
        }
        checked += 1;
    }
    assert!(checked > 0, "no message record in {CLASS}");
    eprintln!("{checked} message records of {CLASS} checked");
}
