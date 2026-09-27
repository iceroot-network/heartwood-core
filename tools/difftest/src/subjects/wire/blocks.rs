//! getBlocks and postBlock: the messages and block lists from the oracle's JSON, and the decoded
//! values in its JSON (amounts as decimal text, transactions and blocks as hex).
//!
//! The readers and writers of the fields that the block and transaction routes share (optional
//! scalars, hex bytes, hex lists and the request headers) are here, and `transactions` uses them.

use heartwood_crypto::utils::hex;
use heartwood_p2p_wire::codecs::Amount;
use heartwood_p2p_wire::codecs::blocks::{self, ListedBlock};
use heartwood_p2p_wire::messages::blocks::{
    BlockHeader, GetBlocksRequest, PostBlockRequest, PostBlockResponse,
};
use heartwood_p2p_wire::messages::shared::Headers;
use heartwood_p2p_wire::{Codec, DecodeError};
use serde_json::{Value, json};

use super::{accepted, bytes_value, object, refused};
use crate::subject::{Documented, Verdict};
use crate::subjects::{Outcome, failed};

/// The payload of `value` for `codec`.
pub(super) fn encode(codec: Codec, value: &Value) -> Outcome<Vec<u8>> {
    Ok(match codec {
        Codec::GetBlocksRequest => blocks::get_blocks_encode_request(&get_blocks_request(value)?),
        Codec::GetBlocksResponse => {
            let listed = value
                .as_array()
                .ok_or_else(|| failed("value: the blocks are not an array"))?
                .iter()
                .map(listed_block)
                .collect::<Outcome<Vec<_>>>()?;
            blocks::get_blocks_encode_response(&listed)
        }
        Codec::PostBlockRequest => blocks::post_block_encode_request(&PostBlockRequest {
            block: field(value, "block", hex_text)?,
            headers: headers(value)?,
            ..PostBlockRequest::default()
        }),
        Codec::PostBlockResponse => blocks::post_block_encode_response(&PostBlockResponse {
            status: field(value, "status", Value::as_bool)?,
            height: field(value, "height", as_u32)?,
            ..PostBlockResponse::default()
        }),
        other => return Err(failed(format!("{other:?} is not a block codec"))),
    })
}

/// The verdict on `bytes` decoded with `codec`; its output is the decoded value.
pub(super) fn decode(codec: Codec, bytes: &[u8]) -> Outcome<Verdict> {
    match codec {
        Codec::GetBlocksRequest => verdict(
            blocks::get_blocks_decode_request(bytes),
            get_blocks_request_value,
        ),
        Codec::GetBlocksResponse => verdict(blocks::get_blocks_decode_response(bytes), |listed| {
            listed.iter().map(listed_block_value).collect()
        }),
        Codec::PostBlockRequest => verdict(blocks::post_block_decode_request(bytes), |request| {
            object(request, |key| match key {
                "block" => Ok(bytes_value(request.block.as_deref().unwrap_or_default())),
                "headers" => headers_value(request.headers.as_ref()),
                other => Err(unknown(other)),
            })
        }),
        Codec::PostBlockResponse => {
            verdict(blocks::post_block_decode_response(bytes), |response| {
                object(response, |key| match key {
                    "status" => Ok(json!(response.status)),
                    "height" => Ok(json!(response.height)),
                    other => Err(unknown(other)),
                })
            })
        }
        other => Err(failed(format!("{other:?} is not a block codec"))),
    }
}

fn get_blocks_request(value: &Value) -> Outcome<GetBlocksRequest> {
    Ok(GetBlocksRequest {
        last_block_height: field(value, "lastBlockHeight", as_u32)?,
        block_limit: field(value, "blockLimit", as_u32)?,
        headers_only: field(value, "headersOnly", Value::as_bool)?,
        serialised: field(value, "serialised", Value::as_bool)?,
        headers: headers(value)?,
        ..GetBlocksRequest::default()
    })
}

fn get_blocks_request_value(request: &GetBlocksRequest) -> Outcome<Value> {
    object(request, |key| match key {
        "lastBlockHeight" => Ok(json!(request.last_block_height)),
        "blockLimit" => Ok(json!(request.block_limit)),
        "headersOnly" => Ok(json!(request.headers_only)),
        "serialised" => Ok(json!(request.serialised)),
        "headers" => headers_value(request.headers.as_ref()),
        other => Err(unknown(other)),
    })
}

/// A block of a getBlocks response as the reference's server hands it to the codec: the header's
/// present keys, the three amounts (always present), and the transactions as hex (none when
/// absent).
fn listed_block(value: &Value) -> Outcome<ListedBlock> {
    let amount = |key: &str| {
        let text = value[key]
            .as_str()
            .ok_or_else(|| failed(format!("value.{key} is not text")))?;
        Amount::parse(text).map_err(|e| failed(format!("value.{key}: {e}")))
    };
    let header = BlockHeader {
        id: field(value, "id", as_string)?,
        version: field(value, "version", as_u32)?,
        timestamp: field(value, "timestamp", as_u32)?,
        previous_block: field(value, "previousBlock", as_string)?,
        height: field(value, "height", as_u32)?,
        number_of_transactions: field(value, "numberOfTransactions", as_u32)?,
        payload_length: field(value, "payloadLength", as_u32)?,
        payload_hash: field(value, "payloadHash", as_string)?,
        generator_public_key: field(value, "generatorPublicKey", as_string)?,
        block_signature: field(value, "blockSignature", as_string)?,
        ..BlockHeader::default()
    };
    Ok(ListedBlock {
        header,
        total_amount: amount("totalAmount")?,
        total_fee: amount("totalFee")?,
        reward: amount("reward")?,
        transactions: hex_list(value, "transactions")?.unwrap_or_default(),
    })
}

/// A decoded block: `{...header, totalAmount, totalFee, reward, transactions}`.
fn listed_block_value(block: &ListedBlock) -> Outcome<Value> {
    let header = &block.header;
    object(header, |key| match key {
        "id" => Ok(json!(header.id)),
        "version" => Ok(json!(header.version)),
        "timestamp" => Ok(json!(header.timestamp)),
        "previousBlock" => Ok(json!(header.previous_block)),
        "height" => Ok(json!(header.height)),
        "numberOfTransactions" => Ok(json!(header.number_of_transactions)),
        "totalAmount" => Ok(json!(block.total_amount.to_string())),
        "totalFee" => Ok(json!(block.total_fee.to_string())),
        "reward" => Ok(json!(block.reward.to_string())),
        "payloadLength" => Ok(json!(header.payload_length)),
        "payloadHash" => Ok(json!(header.payload_hash)),
        "generatorPublicKey" => Ok(json!(header.generator_public_key)),
        "blockSignature" => Ok(json!(header.block_signature)),
        "transactions" => Ok(hex_list_value(&block.transactions)),
        other => Err(unknown(other)),
    })
}

// Shared with `transactions`.

/// The verdict on a decoder's result: the value in the oracle's JSON, or the refusal. Where the
/// oracle's answer is the overflow of the reference's stack on deeply nested groups, the case is
/// the documented difference whatever Heartwood's verdict: Heartwood reads past the groups, and a
/// list can then fail later, on another item.
pub(super) fn verdict<T>(
    result: Result<T, DecodeError>,
    value: impl FnOnce(&T) -> Outcome<Value>,
) -> Outcome<Verdict> {
    Ok(match result {
        Ok(decoded) => accepted(value(&decoded)?),
        Err(error) => Verdict::DiffersWhereShown {
            difference: Documented::DeepGroups,
            verdict: Box::new(refused(error)),
        },
    })
}

/// A decoded object has a key that its message does not declare: a harness bug.
pub(super) fn unknown(key: &str) -> crate::subject::SubjectError {
    failed(format!("the decoded object has an unknown key {key}"))
}

/// The optional field `key` of `value`: `None` when the key is absent or null, else what `read`
/// makes of it.
pub(super) fn field<T>(
    value: &Value,
    key: &str,
    read: impl FnOnce(&Value) -> Option<T>,
) -> Outcome<Option<T>> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(present) => read(present)
            .map(Some)
            .ok_or_else(|| failed(format!("value.{key} has the wrong type: {present}"))),
    }
}

pub(super) fn as_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

pub(super) fn as_string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

pub(super) fn hex_text(value: &Value) -> Option<Vec<u8>> {
    value.as_str().and_then(|text| hex::decode(text).ok())
}

/// The optional list `key` of hex texts.
pub(super) fn hex_list(value: &Value, key: &str) -> Outcome<Option<Vec<Vec<u8>>>> {
    field(value, key, |list| {
        list.as_array()?.iter().map(hex_text).collect()
    })
}

pub(super) fn hex_list_value<T: AsRef<[u8]>>(items: &[T]) -> Value {
    items
        .iter()
        .map(|item| bytes_value(item.as_ref()))
        .collect()
}

/// The request headers `value.headers`: the client's version and port.
pub(super) fn headers(value: &Value) -> Outcome<Option<Headers>> {
    let Some(headers) = value.get("headers").filter(|h| !h.is_null()) else {
        return Ok(None);
    };
    Ok(Some(Headers {
        version: field(headers, "version", as_string)?,
        port: field(headers, "port", as_u32)?,
        ..Headers::default()
    }))
}

pub(super) fn headers_value(headers: Option<&Headers>) -> Outcome<Value> {
    let Some(headers) = headers else {
        return Ok(Value::Null);
    };
    object(headers, |key| match key {
        "version" => Ok(json!(headers.version)),
        "port" => Ok(json!(headers.port)),
        other => Err(unknown(other)),
    })
}
